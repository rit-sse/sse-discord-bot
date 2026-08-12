use poise::serenity_prelude as serenity;
use sse_discord_bot::{Error, commands, config::AppConfig, data_from_config, guild_id, logging};
use std::collections::HashSet;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    logging::load_dotenv()?;
    logging::init();

    let config = AppConfig::from_env()?;
    let intents = serenity::GatewayIntents::non_privileged();
    let bot_token = sse_discord_bot::bot_token(&config);
    let guild_id = guild_id(&config);
    let data = data_from_config(config.clone()).await?;
    let data_config = config.clone();
    tracing::info!(
        guild_id = ?guild_id,
        onboarding_target_count = config.onboarding.targets.len(),
        "starting discord bot..."
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

                Ok(data)
            })
        })
        .build();

    let mut client = serenity::ClientBuilder::new(bot_token, intents)
        .framework(framework)
        .await?;

    tracing::info!("discord client built; connecting to gateway...");
    client.start().await?;
    Ok(())
}

async fn configure_onboard_command_permissions(
    ctx: &serenity::Context,
    guild_id: serenity::GuildId,
    registered_commands: &[serenity::Command],
    config: &AppConfig,
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
