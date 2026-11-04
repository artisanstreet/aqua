use super::{AppIdentity, Notification, NotificationError, NotificationResponse, ResponseHandler};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const DEFAULT_ACTION: &str = "default";
const CLOSED_ACTION: &str = "__closed";
const MAX_ACTIVE: usize = 256;

#[derive(Clone, Copy)]
struct Active {
    generation: u64,
    server_id: Option<u32>,
}

type Registry = Arc<Mutex<HashMap<String, Active>>>;

pub struct NotificationCenter {
    identity: AppIdentity,
    active: Registry,
    generation: u64,
    response: ResponseHandler,
}

fn lock(registry: &Registry) -> std::sync::MutexGuard<'_, HashMap<String, Active>> {
    registry.lock().unwrap_or_else(|error| error.into_inner())
}

fn remove_current(registry: &Registry, tag: &str, generation: u64) -> bool {
    let mut active = lock(registry);
    match active.get(tag) {
        Some(current) if current.generation == generation => active.remove(tag).is_some(),
        _ => false,
    }
}

fn record_server_id(registry: &Registry, tag: &str, generation: u64, server_id: u32) {
    if let Some(current) = lock(registry).get_mut(tag) {
        if current.generation == generation {
            current.server_id = Some(server_id);
        }
    }
}

fn transport_action(index: usize) -> String {
    format!("aqua-action-{index}")
}

fn response_action(
    transport: &str,
    action_ids: &HashMap<String, String>,
) -> Option<Option<String>> {
    match transport {
        DEFAULT_ACTION => Some(None),
        CLOSED_ACTION => None,
        other => action_ids.get(other).cloned().map(Some),
    }
}

impl NotificationCenter {
    pub fn new(
        identity: AppIdentity,
        response: ResponseHandler,
    ) -> Result<Self, NotificationError> {
        Ok(Self {
            identity,
            active: Arc::new(Mutex::new(HashMap::new())),
            generation: 0,
            response,
        })
    }

    pub fn show(&mut self, notification: Notification) -> Result<(), NotificationError> {
        notification.validate()?;
        let replaces = {
            let active = lock(&self.active);
            match active.get(&notification.tag) {
                None if active.len() >= MAX_ACTIVE => return Err(NotificationError::TooManyActive),
                current => current.and_then(|current| current.server_id),
            }
        };
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or_else(|| NotificationError::Native("notification generation exhausted".into()))?;
        let generation = self.generation;

        let mut builder = notify_rust::Notification::new();
        builder
            .appname(self.identity.name())
            .summary(&notification.title)
            .body(&notification.body)
            .action(DEFAULT_ACTION, DEFAULT_ACTION);
        let mut action_ids = HashMap::new();
        for (index, action) in notification.actions.iter().enumerate() {
            let transport = transport_action(index);
            builder.action(&transport, &action.label);
            action_ids.insert(transport, action.id.clone());
        }
        if let Some(server_id) = replaces {
            builder.id(server_id);
        }
        let built = builder.finalize();

        lock(&self.active).insert(
            notification.tag.clone(),
            Active {
                generation,
                server_id: replaces,
            },
        );

        let registry = self.active.clone();
        let response = self.response.clone();
        let tag = notification.tag.clone();
        let spawned = std::thread::Builder::new()
            .name("system-notification".to_owned())
            .spawn(move || match built.show() {
                Ok(handle) => {
                    record_server_id(&registry, &tag, generation, handle.id());
                    handle.wait_for_action(|transport| {
                        match response_action(transport, &action_ids) {
                            Some(action_id) if remove_current(&registry, &tag, generation) => {
                                response(NotificationResponse {
                                    tag: tag.clone(),
                                    action_id,
                                })
                            }
                            _ => {}
                        }
                    });
                    remove_current(&registry, &tag, generation);
                }
                Err(error) => {
                    eprintln!("aqua: system notification failed: {error}");
                    remove_current(&registry, &tag, generation);
                }
            });
        match spawned {
            Ok(_) => Ok(()),
            Err(error) => {
                remove_current(&self.active, &notification.tag, generation);
                Err(NotificationError::Native(error.to_string()))
            }
        }
    }

    pub fn withdraw(&mut self, tag: &str) -> Result<(), NotificationError> {
        lock(&self.active).remove(tag);
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../tests/notifications/linux.rs"]
mod tests;
