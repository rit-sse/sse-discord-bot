use crate::{
    config::EmailConfig,
    verification::{EmailAddress, VerificationCode},
};
use anyhow::{Context, Result, anyhow};
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

    pub async fn send_verification_code(
        &self,
        email: &EmailAddress,
        code: &VerificationCode,
    ) -> Result<()> {
        let from: Mailbox = self
            .config
            .from_address
            .parse()
            .context("failed to parse EMAIL_FROM_ADDRESS as an email mailbox")?;
        let to: Mailbox = email
            .to_string()
            .parse()
            .context("failed to parse verification recipient email as an email mailbox")?;

        let message = Message::builder()
            .from(from.clone())
            .to(to.clone())
            .subject("Your SSE Discord verification code")
            .header(ContentType::TEXT_PLAIN)
            .body(format!(
                "Your SSE Discord verification code is: {code}\n\n\
                This code expires in 1 hour.\n\n\
                Enter this code in the Discord verification prompt to finish verifying your account.\n\n\
                If you did not request this code, you can ignore this email."
            ))
            .context("failed to build verification email message")?;

        let credentials = Credentials::new(
            self.config.smtp_username.expose_secret().to_owned(),
            self.config.smtp_password.expose_secret().to_owned(),
        );

        let mailer = if self.config.smtp_starttls {
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&self.config.smtp_host)?
                .port(self.config.smtp_port)
                .credentials(credentials)
                .build()
        } else {
            AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&self.config.smtp_host)
                .port(self.config.smtp_port)
                .credentials(credentials)
                .build()
        };

        tracing::info!(
            smtp_host = %self.config.smtp_host,
            smtp_port = self.config.smtp_port,
            smtp_starttls = self.config.smtp_starttls,
            from = %from,
            to = %to,
            "sending verification email"
        );

        mailer
            .send(message)
            .await
            .context("failed to send verification email through SMTP")?;

        tracing::info!(
            smtp_host = %self.config.smtp_host,
            smtp_port = self.config.smtp_port,
            smtp_starttls = self.config.smtp_starttls,
            to = %to,
            "sent verification email"
        );

        Ok(())
    }
}
