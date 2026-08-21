use poise::serenity_prelude as serenity;
use sse_discord_bot::{commands, config::AppConfig, data_from_config, guild_id, logging};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    logging::load_dotenv()?;
    logging::init();

    let config = AppConfig::from_env()?;
    let intents =
        serenity::GatewayIntents::non_privileged() | serenity::GatewayIntents::GUILD_MEMBERS;
    let bot_token = sse_discord_bot::bot_token(&config);
    let guild_id = guild_id(&config);
    let enabled_features = config.features.names().join(",");
    let enabled_commands = commands::enabled(&config.features);
    let command_count = enabled_commands.len();
    let data = data_from_config(&config).await?;
    tracing::info!(
        guild_id = ?guild_id,
        enabled_features,
        command_count,
        "starting discord bot..."
    );

    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: enabled_commands,
            event_handler: |ctx, event, _framework, data| {
                Box::pin(async move { commands::handle_event(ctx, event, data).await })
            },
            ..Default::default()
        })
        .setup(move |ctx, _ready, framework| {
            Box::pin(async move {
                if let Some(guild_id) = guild_id {
                    serenity::Command::set_global_commands(ctx, Vec::new()).await?;
                    tracing::info!("cleared global slash commands for guild-scoped deployment");

                    let commands =
                        poise::builtins::create_application_commands(&framework.options().commands);
                    let _registered_commands = guild_id.set_commands(ctx, commands).await?;
                    tracing::info!(%guild_id, "registered guild slash commands");
                    // Discord does not allow bots to update application command permissions.
                    // configure_onboard_command_permissions(
                    //     ctx,
                    //     guild_id,
                    //     &_registered_commands,
                    //     &_data_config,
                    // )
                    // .await?;
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
