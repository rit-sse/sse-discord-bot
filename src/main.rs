use authentik::AuthentikClient;
use email::EmailSender;
use onboarding::OnboardingStore;
use poise::serenity_prelude as serenity;
use secrecy::ExposeSecret;
use std::{collections::HashSet, sync::Mutex};
use verification::{VerificationStore, VerifiedIdentityStore};

pub mod authentik;
pub mod commands;
mod config;
pub mod email;
mod logging;
pub mod onboarding;
pub mod verification;
pub struct Data {
    pub config: config::AppConfig,
    pub email_sender: EmailSender,
    pub verification_store: Mutex<VerificationStore>,
    pub verified_identities: Mutex<VerifiedIdentityStore>,
    pub onboarding_store: Mutex<OnboardingStore>,
    pub authentik_client: AuthentikClient,
} // User data, which is stored and accessible in all command invocations
type Error = Box<dyn std::error::Error + Send + Sync>;
type Context<'a> = poise::Context<'a, Data, Error>;
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    logging::load_dotenv()?;
    logging::init();

    let config = config::AppConfig::from_env()?;
    let intents = serenity::GatewayIntents::non_privileged();
    let bot_token = config.discord.bot_token.expose_secret().to_owned();
    let guild_id = config.discord.guild_id.map(serenity::GuildId::new);
    let email_sender = EmailSender::new(config.email.clone())?;
    let authentik_client = AuthentikClient::new(config.authentik.clone())?;
    let data_config = config.clone();
    tracing::info!(
        guild_id = ?guild_id,
        onboarding_target_count = config.onboarding.targets.len(),
        "starting discord bot"
    );

    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: commands::all(),
            event_handler: |ctx, event, _framework, data| {
                Box::pin(async move { commands::handle_event(ctx, event, data).await })
            },
            ..Default::default()
        })
        .setup(move |ctx, _ready, framework| {
            Box::pin(async move {
                if let Some(guild_id) = guild_id {
                    let commands =
                        poise::builtins::create_application_commands(&framework.options().commands);
                    let registered_commands = guild_id.set_commands(ctx, commands).await?;
                    tracing::info!(%guild_id, "registered guild slash commands");
                    configure_onboard_command_permissions(
                        ctx,
                        guild_id,
                        &registered_commands,
                        &data_config,
                    )
                    .await?;
                } else {
                    poise::builtins::register_globally(ctx, &framework.options().commands).await?;
                    tracing::warn!(
                        "registered global slash commands; role-based command visibility only works for guild commands"
                    );
                }

                Ok(Data {
                    config: data_config,
                    email_sender,
                    verification_store: Mutex::new(VerificationStore::new()),
                    verified_identities: Mutex::new(VerifiedIdentityStore::new()),
                    onboarding_store: Mutex::new(OnboardingStore::new()),
                    authentik_client,
                })
            })
        })
        .build();

    let mut client = serenity::ClientBuilder::new(bot_token, intents)
        .framework(framework)
        .await?;

    tracing::info!("discord client built; connecting to gateway");
    client.start().await?;
    Ok(())
}

async fn configure_onboard_command_permissions(
    ctx: &serenity::Context,
    guild_id: serenity::GuildId,
    registered_commands: &[serenity::Command],
    config: &config::AppConfig,
) -> Result<(), Error> {
    let Some(onboard_command) = registered_commands
        .iter()
        .find(|command| command.name == "onboard")
    else {
        tracing::warn!(%guild_id, "could not find registered onboard command to configure permissions");
        return Ok(());
    };

    let manager_role_ids = config
        .onboarding
        .targets
        .iter()
        .flat_map(|target| target.approver_role_ids.iter().copied())
        .collect::<HashSet<_>>();

    if manager_role_ids.is_empty() {
        tracing::warn!(
            %guild_id,
            command_id = %onboard_command.id,
            "no manager roles configured for onboard command visibility"
        );
        return Ok(());
    }

    let mut permissions = vec![serenity::CreateCommandPermission::everyone(guild_id, false)];
    permissions.extend(manager_role_ids.iter().map(|role_id| {
        serenity::CreateCommandPermission::role(serenity::RoleId::new(*role_id), true)
    }));

    guild_id
        .edit_command_permissions(
            ctx,
            onboard_command.id,
            serenity::EditCommandPermissions::new(permissions),
        )
        .await?;

    tracing::info!(
        %guild_id,
        command_id = %onboard_command.id,
        manager_role_count = manager_role_ids.len(),
        "configured onboard command visibility"
    );

    Ok(())
}
