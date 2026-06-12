use crate::{
    config::EmailConfig,
    verification::{EmailAddress, VerificationCode},
};
use anyhow::{Result, anyhow};
use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    message::{Mailbox, header::ContentType},
    transport::smtp::authentication::Credentials,
};
use secrecy::ExposeSecret;

pub struct EmailSender {
    config: EmailConfig,
}

impl EmailSender {
    pub fn new(config: EmailConfig) -> Result<EmailSender> {
        if config.smtp_host.trim().is_empty() {
            return Err(anyhow!("SMTP_HOST cannot be empty"));
        }

        if config.from_address.trim().is_empty() {
            return Err(anyhow!("EMAIL_FROM_ADDRESS cannot be empty"));
        }

        Ok(Self { config })
    }

    pub async fn send_letter(&self, email: EmailAddress, code: VerificationCode) -> Result<()> {
        let from: Mailbox = self.config.from_address.parse()?;
        let to: Mailbox = email.to_string().parse()?;

        let message = Message::builder()
            .from(from)
            .to(to)
            .subject("Your SSE Discord verification code")
            .header(ContentType::TEXT_PLAIN)
            .body(format!("Your verification code is: {code}"))?;

        let credentials = Credentials::new(
            self.config.smtp_username.expose_secret().to_owned(),
            self.config.smtp_password.expose_secret().to_owned(),
        );

        let mailer = AsyncSmtpTransport::<Tokio1Executor>::relay(&self.config.smtp_host)?
            .port(self.config.smtp_port)
            .credentials(credentials)
            .build();

        mailer.send(message).await?;
        Ok(())
    }
}
