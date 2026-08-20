use crate::{
    Data, Error,
    config::{Feature, FeatureSet},
};

pub mod age;
pub mod onboard;
pub mod verify;

pub fn enabled(features: &FeatureSet) -> Vec<poise::Command<Data, Error>> {
    let mut commands = Vec::new();

    if features.contains(Feature::Age) {
        commands.push(age::age());
    }
    if features.contains(Feature::Verification) {
        commands.push(verify::verify());
    }
    if features.contains(Feature::Onboarding) {
        commands.push(onboard::onboard());
    }

    commands
}

pub async fn handle_event(
    serenity_ctx: &poise::serenity_prelude::Context,
    event: &poise::serenity_prelude::FullEvent,
    data: &Data,
) -> Result<(), Error> {
    if data.modules.onboarding.is_none() {
        return Ok(());
    }

    if let poise::serenity_prelude::FullEvent::InteractionCreate {
        interaction: poise::serenity_prelude::Interaction::Component(component_interaction),
    } = event
    {
        onboard::handle_component_interaction(serenity_ctx, component_interaction, data).await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verification_only_registers_only_verify() {
        let features = "verification"
            .parse::<FeatureSet>()
            .expect("feature set should parse");
        let command_names = enabled(&features)
            .into_iter()
            .map(|command| command.name)
            .collect::<Vec<_>>();

        assert_eq!(command_names, vec!["verify"]);
    }

    #[test]
    fn full_feature_set_registers_each_enabled_command() {
        let features = "age,verification,onboarding"
            .parse::<FeatureSet>()
            .expect("feature set should parse");
        let command_names = enabled(&features)
            .into_iter()
            .map(|command| command.name)
            .collect::<Vec<_>>();

        assert_eq!(command_names, vec!["age", "verify", "onboard"]);
    }
}
