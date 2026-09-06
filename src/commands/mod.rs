use crate::{
    Data, Error,
    config::{Feature, FeatureSet},
};

pub mod age;
pub mod find;
pub mod onboard;
pub mod verify;

pub fn enabled(features: &FeatureSet) -> Vec<poise::Command<Data, Error>> {
    let mut commands = Vec::new();

    if features.contains(Feature::Age) {
        commands.push(age::age());
    }
    if features.contains(Feature::Verification) {
        commands.push(verify::verification_panel());
    }
    if features.contains(Feature::Onboarding) {
        commands.push(onboard::onboard());
        commands.push(onboard::onboarding_status());
        commands.push(onboard::onboarding_retry());
        commands.push(onboard::onboarding_check());
    }

    if features.contains(Feature::Find) {
        commands.push(find::find());
    }

    commands
}

pub async fn handle_event(
    serenity_ctx: &poise::serenity_prelude::Context,
    event: &poise::serenity_prelude::FullEvent,
    data: &Data,
) -> Result<(), Error> {
    use poise::serenity_prelude::{FullEvent, Interaction};

    match event {
        FullEvent::GuildMemberAddition { new_member } if data.modules.verification.is_some() => {
            verify::handle_member_join(serenity_ctx, new_member, data).await?;
        }
        FullEvent::InteractionCreate {
            interaction: Interaction::Component(component_interaction),
        } => {
            if data.modules.verification.is_some() {
                verify::handle_component_interaction(serenity_ctx, component_interaction, data)
                    .await?;
            }
            if data.modules.onboarding.is_some() {
                onboard::handle_component_interaction(serenity_ctx, component_interaction, data)
                    .await?;
            }
        }
        FullEvent::InteractionCreate {
            interaction: Interaction::Modal(modal_interaction),
        } if data.modules.verification.is_some() => {
            verify::handle_modal_interaction(serenity_ctx, modal_interaction, data).await?;
        }
        _ => {}
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verification_only_registers_only_panel() {
        let features = "verification"
            .parse::<FeatureSet>()
            .expect("feature set should parse");
        let command_names = enabled(&features)
            .into_iter()
            .map(|command| command.name)
            .collect::<Vec<_>>();

        assert_eq!(command_names, vec!["verification_panel"]);
    }

    #[test]
    fn full_feature_set_registers_each_enabled_command() {
        let features = "age,verification,onboarding,find"
            .parse::<FeatureSet>()
            .expect("feature set should parse");
        let command_names = enabled(&features)
            .into_iter()
            .map(|command| command.name)
            .collect::<Vec<_>>();

        assert_eq!(
            command_names,
            vec![
                "age",
                "verification_panel",
                "onboard",
                "onboarding_status",
                "onboarding_retry",
                "onboarding_check",
                "find"
            ]
        );
    }

    #[test]
    fn find_alone_registers_a_guild_admin_command_with_a_required_email() {
        let features = "find".parse::<FeatureSet>().expect("find should parse");
        let commands = enabled(&features);
        assert_eq!(commands.len(), 1);
        let command = &commands[0];
        assert_eq!(command.name, "find");
        assert!(command.guild_only);
        assert_eq!(
            command.required_permissions,
            poise::serenity_prelude::Permissions::MANAGE_GUILD
        );
        assert_eq!(
            command.default_member_permissions,
            command.required_permissions
        );
        assert_eq!(command.parameters.len(), 1);
        assert_eq!(command.parameters[0].name, "email");
        assert!(command.parameters[0].required);
    }
}
