use discord_rich_presence::{DiscordIpc, DiscordIpcClient, activity};
use std::sync::mpsc::{self, Sender};

const DISCORD_APP_ID: &str = "1452620752263319665";

enum DiscordCommand {
    Connect,
    Disconnect,
    SetActivity {
        state: String,
        details: String,
        large_image: Option<String>,
        start_timestamp: Option<i64>,
        end_timestamp: Option<i64>,
    },
    ClearActivity,
}

pub struct DiscordRpc {
    commands: Sender<DiscordCommand>,
}

impl DiscordRpc {
    pub fn new() -> Self {
        let (commands, receiver) = mpsc::channel();

        std::thread::spawn(move || {
            let mut client: Option<DiscordIpcClient> = None;

            for command in receiver {
                match command {
                    DiscordCommand::Connect => {
                        if client.is_some() {
                            continue;
                        }

                        let mut next_client = DiscordIpcClient::new(DISCORD_APP_ID);
                        match next_client.connect() {
                            Ok(()) => {
                                tracing::info!("Discord RPC connected successfully");
                                client = Some(next_client);
                            }
                            Err(error) => {
                                tracing::error!("Discord RPC connect error: {error}");
                            }
                        }
                    }
                    DiscordCommand::Disconnect => {
                        if let Some(mut current_client) = client.take() {
                            tracing::info!("Discord RPC disconnecting");
                            if let Err(error) = current_client.close() {
                                tracing::error!("Discord RPC disconnect error: {error}");
                            }
                        }
                    }
                    DiscordCommand::SetActivity {
                        state,
                        details,
                        large_image,
                        start_timestamp,
                        end_timestamp,
                    } => {
                        let Some(current_client) = client.as_mut() else {
                            continue;
                        };

                        let mut payload = activity::Activity::new()
                            .activity_type(activity::ActivityType::Watching)
                            .state(&state)
                            .details(&details);
                        payload = payload.assets(
                            activity::Assets::new()
                                .large_image(large_image.as_deref().unwrap_or("stremio_logo"))
                                .large_text("Stremio"),
                        );

                        let timestamps = match (start_timestamp, end_timestamp) {
                            (Some(start), Some(end)) => {
                                Some(activity::Timestamps::new().start(start).end(end))
                            }
                            (Some(start), None) => Some(activity::Timestamps::new().start(start)),
                            (None, Some(end)) => Some(activity::Timestamps::new().end(end)),
                            (None, None) => None,
                        };
                        if let Some(timestamps) = timestamps {
                            payload = payload.timestamps(timestamps);
                        }

                        if let Err(error) = current_client.set_activity(payload) {
                            tracing::error!("Discord RPC set activity error: {error}");
                            client = None;
                        }
                    }
                    DiscordCommand::ClearActivity => {
                        let Some(current_client) = client.as_mut() else {
                            continue;
                        };

                        if let Err(error) = current_client.clear_activity() {
                            tracing::error!("Discord RPC clear activity error: {error}");
                            client = None;
                        }
                    }
                }
            }

            if let Some(mut current_client) = client {
                let _ = current_client.close();
            }
        });

        Self { commands }
    }

    pub fn connect(&self) -> Result<(), String> {
        self.send(DiscordCommand::Connect)
    }

    pub fn disconnect(&self) -> Result<(), String> {
        self.send(DiscordCommand::Disconnect)
    }

    pub fn set_activity(
        &self,
        state: &str,
        details: &str,
        large_image: Option<&str>,
        start_timestamp: Option<i64>,
        end_timestamp: Option<i64>,
    ) -> Result<(), String> {
        self.send(DiscordCommand::SetActivity {
            state: state.to_string(),
            details: details.to_string(),
            large_image: large_image.map(ToString::to_string),
            start_timestamp,
            end_timestamp,
        })
    }

    pub fn clear_activity(&self) -> Result<(), String> {
        self.send(DiscordCommand::ClearActivity)
    }

    fn send(&self, command: DiscordCommand) -> Result<(), String> {
        self.commands
            .send(command)
            .map_err(|e| format!("Failed to send Discord command: {e}"))
    }
}
