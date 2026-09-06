use poise::{CreateReply, serenity_prelude as serenity};

use crate::{Data, Error, db::verify_repository, domain::verification::EmailAddress};

type ApplicationContext<'a> = poise::ApplicationContext<'a, Data, Error>;

/// Find a Discord account by its verified email address
#[poise::command(
    slash_command,
    guild_only,
    required_permissions = "MANAGE_GUILD",
    default_member_permissions = "MANAGE_GUILD"
)]
pub async fn find(
    ctx: ApplicationContext<'_>,
    #[description = "The member's full verified email address"] email: String,
) -> Result<(), Error> {
    let email = match EmailAddress::parse(&email) {
        Ok(email) => email,
        Err(_) => {
            ctx.send(
                CreateReply::default()
                    .content("That email doesn't look right.")
                    .ephemeral(true),
            )
            .await?;
            return Ok(());
        }
    };

    ctx.defer_ephemeral().await?;
    let user_ids = verify_repository::find_user_ids_by_email(&ctx.data().db, &email).await?;
    let pages = if user_ids.is_empty() {
        vec!["I couldn't find anyone with that email.".to_owned()]
    } else {
        user_ids
            .chunks(20)
            .map(|page| {
                page.iter()
                    .map(|user_id| format!("<@{user_id}>"))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .collect()
    };

    // Split long results into separate private replies rather than dropping duplicate matches.
    for page in pages {
        ctx.send(
            CreateReply::default()
                .content(page)
                .ephemeral(true)
                .allowed_mentions(serenity::CreateAllowedMentions::new()),
        )
        .await?;
    }
    Ok(())
}
