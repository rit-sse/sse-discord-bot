use crate::{Data, Error};

pub mod age;
pub mod onboard;
pub mod verify;

pub fn all() -> Vec<poise::Command<Data, Error>> {
    vec![age::age(), onboard::onboard(), verify::verify()]
}

pub async fn handle_event(
    serenity_ctx: &poise::serenity_prelude::Context,
    event: &poise::serenity_prelude::FullEvent,
    data: &Data,
) -> Result<(), Error> {
    if let poise::serenity_prelude::FullEvent::InteractionCreate {
        interaction: poise::serenity_prelude::Interaction::Component(component_interaction),
    } = event
    {
        onboard::handle_component_interaction(serenity_ctx, component_interaction, data).await?;
    }

    Ok(())
}
