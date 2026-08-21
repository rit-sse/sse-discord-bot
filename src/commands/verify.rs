use crate::{
    Data, Error, VerificationModule,
    db::{
        pending_verification_repository::{
            self, CheckPendingVerificationResult, StartPendingVerificationResult,
        },
        verify_repository,
    },
    domain::verification::{EmailAddress, VerificationCode},
};
use poise::{CreateReply, serenity_prelude as serenity};
use std::time::Duration;

type ApplicationContext<'a> = poise::ApplicationContext<'a, Data, Error>;

const VERIFY_START_BUTTON_ID: &str = "verify:start";
const VERIFY_EMAIL_MODAL_ID: &str = "verify:email";
const VERIFY_EMAIL_INPUT_ID: &str = "verify:email:value";
const VERIFY_CODE_BUTTON_ID: &str = "verify:code";
const VERIFY_CODE_MODAL_ID: &str = "verify:code:modal";
const VERIFY_CODE_INPUT_ID: &str = "verify:code:value";

#[derive(Debug, poise::Modal)]
#[name = "Enter verification code"]
struct VerificationCodeModal {
    #[name = "Verification code"]
    #[placeholder = "123456"]
    #[min_length = 6]
    #[max_length = 6]
    code: String,
}

async fn ephemeral_reply(
    ctx: ApplicationContext<'_>,
    content: impl Into<String>,
) -> Result<(), Error> {
    ctx.send(CreateReply::default().content(content).ephemeral(true))
        .await?;
    Ok(())
}

fn verification_module(data: &Data) -> Result<&VerificationModule, Error> {
    data.modules
        .verification
        .as_ref()
        .ok_or_else(|| "verification is disabled".into())
}

fn parse_authorized_email(
    verification: &VerificationModule,
    input: &str,
) -> Result<EmailAddress, String> {
    let email = EmailAddress::parse(input)
        .map_err(|error| format!("That does not look like a valid email: {error}"))?;

    if verification
        .config
        .allowed_email_domains
        .iter()
        .any(|allowed_domain| email.domain() == allowed_domain)
    {
        return Ok(email);
    }

    let allowed_domains = verification
        .config
        .allowed_email_domains
        .iter()
        .map(|domain| format!("@{domain}"))
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!(
        "Verification requires an email address from: {allowed_domains}"
    ))
}

async fn start_verification(
    data: &Data,
    guild_id: serenity::GuildId,
    user_id: serenity::UserId,
    email: &EmailAddress,
) -> Result<StartPendingVerificationResult, Error> {
    let verification = verification_module(data)?;
    let code = VerificationCode::generate();
    let code_hash = code.hash()?;
    let start_result =
        pending_verification_repository::start_or_reuse(&data.db, user_id.get(), email, &code_hash)
            .await?;

    match start_result {
        StartPendingVerificationResult::Created => {
            if let Err(error) = verification
                .email_sender
                .send_verification_code(email, &code)
                .await
            {
                tracing::error!(
                    user_id = %user_id,
                    guild_id = %guild_id,
                    email = %email,
                    error = %format!("{error:#}"),
                    "verification email delivery failed"
                );

                if let Err(cleanup_error) = pending_verification_repository::delete_if_code_matches(
                    &data.db,
                    user_id.get(),
                    &code_hash,
                )
                .await
                {
                    tracing::error!(
                        user_id = %user_id,
                        guild_id = %guild_id,
                        error = %cleanup_error,
                        "failed to clean up verification after email delivery failure"
                    );
                }

                return Err(error.into());
            }

            tracing::info!(
                user_id = %user_id,
                guild_id = %guild_id,
                email = %email,
                "created verification attempt and sent verification email"
            );
        }
        StartPendingVerificationResult::Reused => {
            tracing::info!(
                user_id = %user_id,
                guild_id = %guild_id,
                "reusing pending verification attempt"
            );
        }
    }

    Ok(start_result)
}

async fn sync_verified_roles(
    serenity_ctx: &serenity::Context,
    member: &serenity::Member,
    verification: &VerificationModule,
) -> Result<(), Error> {
    let verified_role_id = serenity::RoleId::new(verification.config.verified_role_id);
    if !member.roles.contains(&verified_role_id) {
        member.add_role(serenity_ctx, verified_role_id).await?;
    }

    if let Some(unverified_role_id) = verification.config.unverified_role_id {
        let unverified_role_id = serenity::RoleId::new(unverified_role_id);
        if member.roles.contains(&unverified_role_id) {
            member.remove_role(serenity_ctx, unverified_role_id).await?;
        }
    }

    Ok(())
}

async fn complete_verification(
    serenity_ctx: &serenity::Context,
    data: &Data,
    guild_id: serenity::GuildId,
    user: &serenity::User,
    verified_email: &EmailAddress,
) -> Result<(), Error> {
    let verification = verification_module(data)?;
    let role_id = serenity::RoleId::new(verification.config.verified_role_id);
    let member = guild_id.member(serenity_ctx, user.id).await?;
    sync_verified_roles(serenity_ctx, &member, verification).await?;

    if let Some(log_channel_id) = verification.config.log_channel_id {
        let log_channel_id = serenity::ChannelId::new(log_channel_id);
        let message = serenity::CreateMessage::new().embed(
            serenity::CreateEmbed::new()
                .title("Member verified")
                .field(
                    "Discord user",
                    format!("{} ({})", user.name, user.id),
                    false,
                )
                .field("Email", verified_email.to_string(), false)
                .field("Role ID", role_id.to_string(), false)
                .color(0x57_F2_87),
        );

        match log_channel_id.send_message(serenity_ctx, message).await {
            Ok(_) => tracing::info!(
                user_id = %user.id,
                guild_id = %guild_id,
                log_channel_id = %log_channel_id,
                "posted verification audit record"
            ),
            Err(error) => tracing::error!(
                user_id = %user.id,
                guild_id = %guild_id,
                log_channel_id = %log_channel_id,
                error = %error,
                "failed to post verification audit record"
            ),
        }
    }

    tracing::info!(
        user_id = %user.id,
        guild_id = %guild_id,
        role_id = %role_id,
        email = %verified_email,
        "verified user"
    );
    Ok(())
}

async fn prompt_retry_modal(
    ctx: ApplicationContext<'_>,
    user_id: u64,
    attempts_remaining: u8,
) -> Result<Option<VerificationCodeModal>, Error> {
    let custom_id = format!("verify_retry:{user_id}");
    let components = vec![serenity::CreateActionRow::Buttons(vec![
        serenity::CreateButton::new(&custom_id)
            .label("Try again")
            .style(serenity::ButtonStyle::Primary),
    ])];

    ctx.send(
        CreateReply::default()
            .content(format!(
                "That verification code is incorrect. You have {attempts_remaining} attempts remaining."
            ))
            .components(components)
            .ephemeral(true),
    )
    .await?;

    let Some(interaction) = serenity::ComponentInteractionCollector::new(ctx.serenity_context())
        .timeout(Duration::from_secs(60 * 60))
        .filter(move |interaction| {
            interaction.data.custom_id == custom_id && interaction.user.id.get() == user_id
        })
        .await
    else {
        tracing::info!(user_id, "verification retry button timed out");
        return Ok(None);
    };

    let modal_data = poise::execute_modal_on_component_interaction::<VerificationCodeModal>(
        ctx,
        interaction,
        None,
        Some(Duration::from_secs(60 * 60)),
    )
    .await?;

    Ok(modal_data)
}

/// Starts email verification by generating a one-time code.
#[poise::command(slash_command)]
pub async fn verify(ctx: ApplicationContext<'_>, email: String) -> std::result::Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        ephemeral_reply(ctx, "Verification only works inside a server.").await?;
        return Ok(());
    };

    let Some(verification) = ctx.data().modules.verification.as_ref() else {
        tracing::error!("verification command invoked while verification is disabled");
        ephemeral_reply(ctx, "Verification is currently disabled.").await?;
        return Ok(());
    };

    let user = ctx.author();
    let user_id = user.id.get();
    let role_id = serenity::RoleId::new(verification.config.verified_role_id);
    let has_verified_role = user
        .has_role(ctx.serenity_context(), guild_id, role_id)
        .await?;
    let persisted_identity = verify_repository::find_user_by_id(&ctx.data().db, user_id).await?;

    if let Some(identity) = persisted_identity {
        let member = guild_id.member(ctx.serenity_context(), user.id).await?;
        sync_verified_roles(ctx.serenity_context(), &member, verification).await?;
        if !has_verified_role {
            tracing::info!(
                user_id = %user.id,
                guild_id = %guild_id,
                role_id = %role_id,
                email = %identity.email(),
                "restored verified role from persisted identity"
            );
        } else {
            tracing::info!(user_id = %user.id, guild_id = %guild_id, "user is already verified");
        }

        ephemeral_reply(ctx, "You are already verified.").await?;
        return Ok(());
    }

    if has_verified_role {
        tracing::info!(
            user_id = %user.id,
            guild_id = %guild_id,
            "user has verified role but no recorded verified identity; refreshing verification"
        );
    }

    let email = match parse_authorized_email(verification, &email) {
        Ok(email) => email,
        Err(message) => {
            tracing::warn!(
                user_id = %user.id,
                guild_id = %guild_id,
                submitted_email = %email,
                "rejected verification email"
            );
            ephemeral_reply(ctx, message).await?;
            return Ok(());
        }
    };

    start_verification(ctx.data(), guild_id, user.id, &email).await?;

    let Some(modal_data) = ({
        poise::execute_modal::<_, _, VerificationCodeModal>(
            ctx,
            None,
            Some(Duration::from_secs(60 * 60)),
        )
        .await?
    }) else {
        tracing::info!(user_id = %user.id, guild_id = %guild_id, "verification modal timed out or was dismissed");
        return Ok(());
    };

    let mut submitted_code = modal_data.code;
    let verified_email = loop {
        let verification_result =
            pending_verification_repository::check_code(&ctx.data().db, user_id, &submitted_code)
                .await?;

        match verification_result {
            CheckPendingVerificationResult::Accepted { email } => break email,
            CheckPendingVerificationResult::Missing => {
                ephemeral_reply(
                    ctx,
                    "I could not find a pending verification attempt. Please run `/verify` again.",
                )
                .await?;
                return Ok(());
            }
            CheckPendingVerificationResult::Expired => {
                tracing::info!(user_id = %user.id, guild_id = %guild_id, "verification attempt expired");
                ephemeral_reply(
                    ctx,
                    "That verification code expired. Please run `/verify` again.",
                )
                .await?;
                return Ok(());
            }
            CheckPendingVerificationResult::Incorrect {
                attempts_remaining,
                has_attempts_remaining,
            } => {
                tracing::info!(
                    user_id = %user.id,
                    guild_id = %guild_id,
                    attempts_remaining,
                    "submitted incorrect verification code"
                );

                if has_attempts_remaining {
                    let Some(modal_data) =
                        prompt_retry_modal(ctx, user_id, attempts_remaining).await?
                    else {
                        return Ok(());
                    };

                    submitted_code = modal_data.code;
                } else {
                    ephemeral_reply(
                        ctx,
                        "That verification code is incorrect, and you have no attempts remaining. Please run `/verify` again to request a new code.",
                    )
                    .await?;
                    return Ok(());
                }
            }
        }
    };

    complete_verification(
        ctx.serenity_context(),
        ctx.data(),
        guild_id,
        user,
        &verified_email,
    )
    .await?;
    ephemeral_reply(ctx, "You have been successfully verified.").await?;
    Ok(())
}

/// Posts the persistent verification button in the current channel.
#[poise::command(
    slash_command,
    required_permissions = "MANAGE_GUILD",
    default_member_permissions = "MANAGE_GUILD"
)]
pub async fn verification_panel(ctx: ApplicationContext<'_>) -> Result<(), Error> {
    if ctx.guild_id().is_none() {
        ephemeral_reply(
            ctx,
            "The verification panel can only be posted in a server.",
        )
        .await?;
        return Ok(());
    }

    ctx.channel_id()
        .send_message(
            ctx.serenity_context(),
            serenity::CreateMessage::new()
                .content(
                    "Verify your membership using your authorized email address. Your address is only used to confirm your identity.",
                )
                .components(vec![serenity::CreateActionRow::Buttons(vec![
                    serenity::CreateButton::new(VERIFY_START_BUTTON_ID)
                        .label("Verify with RIT Email")
                        .style(serenity::ButtonStyle::Success),
                ])]),
        )
        .await?;

    ephemeral_reply(ctx, "Verification panel posted.").await?;
    Ok(())
}

pub async fn handle_component_interaction(
    serenity_ctx: &serenity::Context,
    interaction: &serenity::ComponentInteraction,
    data: &Data,
) -> Result<(), Error> {
    match interaction.data.custom_id.as_str() {
        VERIFY_START_BUTTON_ID => {
            if data.modules.verification.is_none() {
                interaction
                    .create_response(
                        serenity_ctx,
                        serenity::CreateInteractionResponse::Message(
                            serenity::CreateInteractionResponseMessage::new()
                                .content("Verification is currently disabled.")
                                .ephemeral(true),
                        ),
                    )
                    .await?;
                return Ok(());
            }

            interaction
                .create_response(
                    serenity_ctx,
                    serenity::CreateInteractionResponse::Modal(
                        serenity::CreateModal::new(VERIFY_EMAIL_MODAL_ID, "Verify your email")
                            .components(vec![serenity::CreateActionRow::InputText(
                                serenity::CreateInputText::new(
                                    serenity::InputTextStyle::Short,
                                    "RIT email address",
                                    VERIFY_EMAIL_INPUT_ID,
                                )
                                .placeholder("abc1234@rit.edu")
                                .max_length(254),
                            )]),
                    ),
                )
                .await?;
        }
        VERIFY_CODE_BUTTON_ID => {
            interaction
                .create_response(
                    serenity_ctx,
                    serenity::CreateInteractionResponse::Modal(
                        serenity::CreateModal::new(VERIFY_CODE_MODAL_ID, "Enter verification code")
                            .components(vec![serenity::CreateActionRow::InputText(
                                serenity::CreateInputText::new(
                                    serenity::InputTextStyle::Short,
                                    "Verification code",
                                    VERIFY_CODE_INPUT_ID,
                                )
                                .placeholder("123456")
                                .min_length(6)
                                .max_length(6),
                            )]),
                    ),
                )
                .await?;
        }
        _ => {}
    }

    Ok(())
}

pub async fn handle_modal_interaction(
    serenity_ctx: &serenity::Context,
    interaction: &serenity::ModalInteraction,
    data: &Data,
) -> Result<(), Error> {
    match interaction.data.custom_id.as_str() {
        VERIFY_EMAIL_MODAL_ID => {
            handle_email_modal(serenity_ctx, interaction, data).await?;
        }
        VERIFY_CODE_MODAL_ID => {
            handle_code_modal(serenity_ctx, interaction, data).await?;
        }
        _ => {}
    }

    Ok(())
}

async fn handle_email_modal(
    serenity_ctx: &serenity::Context,
    interaction: &serenity::ModalInteraction,
    data: &Data,
) -> Result<(), Error> {
    interaction
        .create_response(
            serenity_ctx,
            serenity::CreateInteractionResponse::Defer(
                serenity::CreateInteractionResponseMessage::new().ephemeral(true),
            ),
        )
        .await?;

    let Some(guild_id) = interaction.guild_id else {
        edit_modal_response(
            serenity_ctx,
            interaction,
            "Verification only works inside a server.",
            Vec::new(),
        )
        .await?;
        return Ok(());
    };
    let verification = verification_module(data)?;
    let user_id = interaction.user.id;

    if let Some(identity) = verify_repository::find_user_by_id(&data.db, user_id.get()).await? {
        let member = guild_id.member(serenity_ctx, user_id).await?;
        sync_verified_roles(serenity_ctx, &member, verification).await?;
        tracing::info!(
            user_id = %user_id,
            guild_id = %guild_id,
            email = %identity.email(),
            "restored verified roles from persisted identity"
        );
        edit_modal_response(
            serenity_ctx,
            interaction,
            "You are already verified.",
            Vec::new(),
        )
        .await?;
        return Ok(());
    }

    let Some(email_input) = modal_input(interaction, VERIFY_EMAIL_INPUT_ID) else {
        tracing::warn!(user_id = %user_id, guild_id = %guild_id, "verification email modal was missing its input");
        edit_modal_response(
            serenity_ctx,
            interaction,
            "The email form was incomplete. Please try again.",
            Vec::new(),
        )
        .await?;
        return Ok(());
    };

    let email = match parse_authorized_email(verification, email_input) {
        Ok(email) => email,
        Err(message) => {
            tracing::warn!(
                user_id = %user_id,
                guild_id = %guild_id,
                submitted_email = %email_input,
                "rejected verification email"
            );
            edit_modal_response(serenity_ctx, interaction, message, Vec::new()).await?;
            return Ok(());
        }
    };

    let start_result = match start_verification(data, guild_id, user_id, &email).await {
        Ok(result) => result,
        Err(error) => {
            tracing::error!(
                user_id = %user_id,
                guild_id = %guild_id,
                error = %format!("{error:#}"),
                "failed to start verification from button flow"
            );
            edit_modal_response(
                serenity_ctx,
                interaction,
                "I could not send the verification email. Please try again later.",
                Vec::new(),
            )
            .await?;
            return Ok(());
        }
    };

    let content = match start_result {
        StartPendingVerificationResult::Created => {
            "I sent your verification code. Check your email, then enter it below."
        }
        StartPendingVerificationResult::Reused => {
            "You already have an active verification code. Check your email, then enter it below."
        }
    };
    edit_modal_response(
        serenity_ctx,
        interaction,
        content,
        verification_code_button("Enter code"),
    )
    .await?;
    Ok(())
}

async fn handle_code_modal(
    serenity_ctx: &serenity::Context,
    interaction: &serenity::ModalInteraction,
    data: &Data,
) -> Result<(), Error> {
    interaction
        .create_response(
            serenity_ctx,
            serenity::CreateInteractionResponse::Defer(
                serenity::CreateInteractionResponseMessage::new().ephemeral(true),
            ),
        )
        .await?;

    let Some(guild_id) = interaction.guild_id else {
        edit_modal_response(
            serenity_ctx,
            interaction,
            "Verification only works inside a server.",
            Vec::new(),
        )
        .await?;
        return Ok(());
    };
    let user_id = interaction.user.id;
    let Some(code) = modal_input(interaction, VERIFY_CODE_INPUT_ID) else {
        edit_modal_response(
            serenity_ctx,
            interaction,
            "The code form was incomplete. Please try again.",
            verification_code_button("Enter code"),
        )
        .await?;
        return Ok(());
    };

    match pending_verification_repository::check_code(&data.db, user_id.get(), code).await? {
        CheckPendingVerificationResult::Accepted { email } => {
            if let Err(error) =
                complete_verification(serenity_ctx, data, guild_id, &interaction.user, &email).await
            {
                tracing::error!(
                    user_id = %user_id,
                    guild_id = %guild_id,
                    error = %format!("{error:#}"),
                    "verification persisted but Discord role synchronization failed"
                );
                edit_modal_response(
                    serenity_ctx,
                    interaction,
                    "Your identity was verified, but I could not update your Discord roles. Please ask an officer for help or press Verify again.",
                    Vec::new(),
                )
                .await?;
                return Ok(());
            }

            edit_modal_response(
                serenity_ctx,
                interaction,
                "You have been successfully verified.",
                Vec::new(),
            )
            .await?;
        }
        CheckPendingVerificationResult::Missing => {
            edit_modal_response(
                serenity_ctx,
                interaction,
                "I could not find a pending verification attempt. Press Verify with RIT Email to start again.",
                Vec::new(),
            )
            .await?;
        }
        CheckPendingVerificationResult::Expired => {
            tracing::info!(user_id = %user_id, guild_id = %guild_id, "verification attempt expired");
            edit_modal_response(
                serenity_ctx,
                interaction,
                "That verification code expired. Press Verify with RIT Email to request a new one.",
                Vec::new(),
            )
            .await?;
        }
        CheckPendingVerificationResult::Incorrect {
            attempts_remaining,
            has_attempts_remaining,
        } => {
            tracing::info!(
                user_id = %user_id,
                guild_id = %guild_id,
                attempts_remaining,
                "submitted incorrect verification code"
            );
            if has_attempts_remaining {
                edit_modal_response(
                    serenity_ctx,
                    interaction,
                    format!(
                        "That verification code is incorrect. You have {attempts_remaining} attempts remaining."
                    ),
                    verification_code_button("Try again"),
                )
                .await?;
            } else {
                edit_modal_response(
                    serenity_ctx,
                    interaction,
                    "That code is incorrect, and you have no attempts remaining. Press Verify with RIT Email to request a new code.",
                    Vec::new(),
                )
                .await?;
            }
        }
    }

    Ok(())
}

fn modal_input<'a>(
    interaction: &'a serenity::ModalInteraction,
    custom_id: &str,
) -> Option<&'a str> {
    interaction
        .data
        .components
        .iter()
        .flat_map(|row| &row.components)
        .find_map(|component| match component {
            serenity::ActionRowComponent::InputText(input) if input.custom_id == custom_id => {
                input.value.as_deref()
            }
            _ => None,
        })
}

fn verification_code_button(label: &str) -> Vec<serenity::CreateActionRow> {
    vec![serenity::CreateActionRow::Buttons(vec![
        serenity::CreateButton::new(VERIFY_CODE_BUTTON_ID)
            .label(label)
            .style(serenity::ButtonStyle::Primary),
    ])]
}

async fn edit_modal_response(
    serenity_ctx: &serenity::Context,
    interaction: &serenity::ModalInteraction,
    content: impl Into<String>,
    components: Vec<serenity::CreateActionRow>,
) -> Result<(), Error> {
    interaction
        .edit_response(
            serenity_ctx,
            serenity::EditInteractionResponse::new()
                .content(content)
                .components(components),
        )
        .await?;
    Ok(())
}

pub async fn handle_member_join(
    serenity_ctx: &serenity::Context,
    member: &serenity::Member,
    data: &Data,
) -> Result<(), Error> {
    if member.user.bot {
        return Ok(());
    }

    let verification = verification_module(data)?;
    let user_id = member.user.id;
    let guild_id = member.guild_id;
    if let Some(identity) = verify_repository::find_user_by_id(&data.db, user_id.get()).await? {
        sync_verified_roles(serenity_ctx, member, verification).await?;
        tracing::info!(
            user_id = %user_id,
            guild_id = %guild_id,
            email = %identity.email(),
            "restored verified roles for returning member"
        );
        return Ok(());
    }

    if let Some(unverified_role_id) = verification.config.unverified_role_id {
        let unverified_role_id = serenity::RoleId::new(unverified_role_id);
        if !member.roles.contains(&unverified_role_id) {
            member.add_role(serenity_ctx, unverified_role_id).await?;
            tracing::info!(
                user_id = %user_id,
                guild_id = %guild_id,
                role_id = %unverified_role_id,
                "assigned unverified role to new member"
            );
        }
    }

    Ok(())
}
