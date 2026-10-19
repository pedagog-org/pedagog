//! Sending mail: SMTP to a relay (with or without a login and TLS), or, in
//! tests, an outbox in memory.

use lettre::message::{Mailbox, header::ContentType};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

use crate::config::{Smtp, TlsMode};

/// A plain-text message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mail {
    pub to: String,
    pub subject: String,
    pub body: String,
}

#[derive(Clone)]
pub enum Mailer {
    Smtp(Box<SmtpMailer>),
    /// Keeps what was sent (tests).
    #[cfg(test)]
    Memory(std::sync::Arc<std::sync::Mutex<Vec<Mail>>>),
}

#[derive(Clone)]
pub struct SmtpMailer {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
    reply_to: Option<Mailbox>,
}

impl Mailer {
    pub fn smtp(cfg: &Smtp) -> anyhow::Result<Self> {
        let builder = match cfg.tls {
            TlsMode::None => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&cfg.host),
            TlsMode::StartTls => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&cfg.host)?,
            TlsMode::Tls => AsyncSmtpTransport::<Tokio1Executor>::relay(&cfg.host)?,
        };
        let mut builder = builder.port(cfg.port);
        if let Some((user, password)) = &cfg.login {
            builder = builder.credentials(Credentials::new(user.clone(), password.clone()));
        }
        let from = Mailbox::new(Some(cfg.from_name.clone()), cfg.from_address.parse()?);
        let reply_to = cfg.reply_to.as_deref().map(str::parse).transpose()?;
        Ok(Mailer::Smtp(Box::new(SmtpMailer {
            transport: builder.build(),
            from,
            reply_to,
        })))
    }

    #[cfg(test)]
    pub fn memory() -> (Self, std::sync::Arc<std::sync::Mutex<Vec<Mail>>>) {
        let outbox = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        (Mailer::Memory(outbox.clone()), outbox)
    }

    pub async fn send(&self, mail: Mail) -> anyhow::Result<()> {
        match self {
            Mailer::Smtp(smtp) => {
                let SmtpMailer {
                    transport,
                    from,
                    reply_to,
                } = smtp.as_ref();
                let mut message = Message::builder()
                    .from(from.clone())
                    .to(mail.to.parse()?)
                    .subject(mail.subject)
                    .header(ContentType::TEXT_PLAIN);
                if let Some(reply_to) = reply_to {
                    message = message.reply_to(reply_to.clone());
                }
                transport.send(message.body(mail.body)?).await?;
                Ok(())
            }
            #[cfg(test)]
            Mailer::Memory(outbox) => {
                outbox
                    .lock()
                    .map_err(|_| anyhow::anyhow!("outbox poisoned"))?
                    .push(mail);
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn smtp(tls: TlsMode) -> Smtp {
        Smtp {
            host: "relay.ufl.edu".into(),
            port: 25,
            tls,
            login: None,
            from_name: "Pedagog".into(),
            from_address: "pedagog@ufl.edu".into(),
            reply_to: Some("help@ufl.edu".into()),
        }
    }

    #[test]
    fn every_tls_mode_builds_a_transport() {
        for tls in [TlsMode::None, TlsMode::StartTls, TlsMode::Tls] {
            assert!(Mailer::smtp(&smtp(tls)).is_ok(), "{tls:?}");
        }
        let bad = Smtp {
            from_address: "not an address".into(),
            ..smtp(TlsMode::None)
        };
        assert!(Mailer::smtp(&bad).is_err());
    }

    #[tokio::test]
    async fn the_memory_mailer_keeps_what_was_sent() {
        let (mailer, outbox) = Mailer::memory();
        let mail = Mail {
            to: "alice@ufl.edu".into(),
            subject: "s".into(),
            body: "b".into(),
        };
        mailer.send(mail.clone()).await.unwrap();
        assert_eq!(*outbox.lock().unwrap(), [mail]);
    }
}
