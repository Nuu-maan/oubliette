use crate::{Error, Result};
use serenity::all::{
    ChannelId, ChannelType, CreateAttachment, CreateChannel, CreateMessage, EditMessage, GuildId,
    MessageId,
};
use serenity::http::Http;
use std::sync::Arc;

#[derive(Clone)]
pub struct DiscordClient {
    pub http: Arc<Http>,
    pub guild_id: GuildId,
}

pub struct ChannelInfo {
    pub id: u64,
    pub name: String,
}

impl DiscordClient {
    pub fn new(token: &str, guild_id: u64) -> Self {
        Self {
            http: Arc::new(Http::new(token)),
            guild_id: GuildId::new(guild_id),
        }
    }

    pub async fn verify_token(&self) -> Result<String> {
        let me = self.http.get_current_user().await?;
        Ok(me.name.clone())
    }

    pub async fn create_category(&self, name: &str) -> Result<u64> {
        let ch = self
            .guild_id
            .create_channel(
                &*self.http,
                CreateChannel::new(name).kind(ChannelType::Category),
            )
            .await?;
        Ok(ch.id.get())
    }

    pub async fn create_text_channel(&self, name: &str, parent: Option<u64>) -> Result<u64> {
        let mut builder = CreateChannel::new(name).kind(ChannelType::Text);
        if let Some(p) = parent {
            builder = builder.category(ChannelId::new(p));
        }
        let ch = self.guild_id.create_channel(&*self.http, builder).await?;
        Ok(ch.id.get())
    }

    pub async fn upload_chunk(
        &self,
        channel: u64,
        filename: &str,
        bytes: Vec<u8>,
    ) -> Result<MessageId> {
        let attachment = CreateAttachment::bytes(bytes, filename);
        let msg = CreateMessage::new().add_file(attachment);
        let posted = ChannelId::new(channel)
            .send_message(&*self.http, msg)
            .await?;
        Ok(posted.id)
    }

    pub async fn download_chunk(&self, channel: u64, message: u64) -> Result<Vec<u8>> {
        let msg = ChannelId::new(channel)
            .message(&*self.http, MessageId::new(message))
            .await?;
        let att = msg
            .attachments
            .first()
            .ok_or_else(|| Error::Other(format!("no attachment on msg {message}")))?;
        let bytes = reqwest::get(&att.url).await?.bytes().await?;
        Ok(bytes.to_vec())
    }

    pub async fn post_text(&self, channel: u64, content: String) -> Result<MessageId> {
        let msg = CreateMessage::new().content(content);
        let posted = ChannelId::new(channel)
            .send_message(&*self.http, msg)
            .await?;
        Ok(posted.id)
    }

    pub async fn edit_text(&self, channel: u64, message: u64, content: String) -> Result<()> {
        let edit = EditMessage::new().content(content);
        ChannelId::new(channel)
            .edit_message(&*self.http, MessageId::new(message), edit)
            .await?;
        Ok(())
    }

    pub async fn fetch_text(&self, channel: u64, message: u64) -> Result<String> {
        let msg = ChannelId::new(channel)
            .message(&*self.http, MessageId::new(message))
            .await?;
        Ok(msg.content)
    }

    pub async fn fetch_message_body(&self, channel: u64, message: u64) -> Result<Vec<u8>> {
        let msg = ChannelId::new(channel)
            .message(&*self.http, MessageId::new(message))
            .await?;
        if let Some(att) = msg.attachments.first() {
            let bytes = reqwest::get(&att.url).await?.bytes().await?;
            Ok(bytes.to_vec())
        } else {
            Ok(msg.content.into_bytes())
        }
    }
}
