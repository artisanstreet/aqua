use super::{
    native_tag, AppIdentity, Notification, NotificationError, NotificationResponse, ResponseHandler,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use windows::core::{h, IInspectable, Interface, HSTRING};
use windows::Data::Xml::Dom::XmlDocument;
use windows::Foundation::TypedEventHandler;
use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
use windows::UI::Notifications::{
    ToastActivatedEventArgs, ToastDismissedEventArgs, ToastFailedEventArgs, ToastNotification,
    ToastNotificationManager, ToastNotifier,
};

type Active = Arc<Mutex<HashMap<String, (u64, ToastNotification)>>>;

pub struct NotificationCenter {
    identity: HSTRING,
    notifier: ToastNotifier,
    active: Active,
    generation: u64,
    response: ResponseHandler,
    _apartment: crate::apartment::Apartment,
}

fn native_error(error: impl std::fmt::Display) -> NotificationError {
    NotificationError::Native(error.to_string())
}

fn remove_current<T>(
    active: &Arc<Mutex<HashMap<String, (u64, T)>>>,
    tag: &str,
    generation: u64,
) -> bool {
    let removed = {
        let mut active = active.lock().unwrap_or_else(|error| error.into_inner());
        if active
            .get(tag)
            .is_some_and(|(current, _)| *current == generation)
        {
            active.remove(tag)
        } else {
            None
        }
    };
    let matched = removed.is_some();
    drop(removed);
    matched
}

impl NotificationCenter {
    pub fn new(
        identity: AppIdentity,
        response: ResponseHandler,
    ) -> Result<Self, NotificationError> {
        let apartment = crate::apartment::Apartment::new().map_err(native_error)?;
        let key = windows_registry::CURRENT_USER
            .create(format!(
                r"Software\Classes\AppUserModelId\{}",
                identity.identifier()
            ))
            .map_err(native_error)?;
        key.set_string("DisplayName", identity.name())
            .map_err(native_error)?;
        let identity = HSTRING::from(identity.identifier());
        unsafe { SetCurrentProcessExplicitAppUserModelID(&identity) }.map_err(native_error)?;
        let notifier =
            ToastNotificationManager::CreateToastNotifierWithId(&identity).map_err(native_error)?;
        Ok(Self {
            identity,
            notifier,
            active: Arc::new(Mutex::new(HashMap::new())),
            generation: 0,
            response,
            _apartment: apartment,
        })
    }

    pub fn show(&mut self, notification: Notification) -> Result<(), NotificationError> {
        let xml = notification.xml()?;
        {
            let active = self
                .active
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if active.len() >= 256 && !active.contains_key(&notification.tag) {
                return Err(NotificationError::TooManyActive);
            }
        }
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or_else(|| NotificationError::Native("notification generation exhausted".into()))?;
        let generation = self.generation;
        let document = XmlDocument::new().map_err(native_error)?;
        document
            .LoadXml(&HSTRING::from(xml))
            .map_err(native_error)?;
        let toast = ToastNotification::CreateToastNotification(&document).map_err(native_error)?;
        toast
            .SetTag(&HSTRING::from(notification.native_tag()))
            .map_err(native_error)?;
        toast.SetGroup(h!("aqua")).map_err(native_error)?;

        let active = Arc::downgrade(&self.active);
        let tag = notification.tag.clone();
        let response = self.response.clone();
        toast
            .Activated(&TypedEventHandler::<ToastNotification, IInspectable>::new(
                move |_, arguments| {
                    let current = active
                        .upgrade()
                        .is_some_and(|active| remove_current(&active, &tag, generation));
                    if !current {
                        return Ok(());
                    }
                    let action_id = arguments
                        .as_ref()
                        .and_then(|arguments| arguments.cast::<ToastActivatedEventArgs>().ok())
                        .and_then(|arguments| arguments.Arguments().ok())
                        .filter(|arguments| !arguments.is_empty())
                        .map(|arguments| arguments.to_string());
                    response(NotificationResponse {
                        tag: tag.clone(),
                        action_id,
                    });
                    Ok(())
                },
            ))
            .map_err(native_error)?;

        let active = Arc::downgrade(&self.active);
        let tag = notification.tag.clone();
        toast
            .Dismissed(&TypedEventHandler::<
                ToastNotification,
                ToastDismissedEventArgs,
            >::new(move |_, _| {
                if let Some(active) = active.upgrade() {
                    remove_current(&active, &tag, generation);
                }
                Ok(())
            }))
            .map_err(native_error)?;

        let active = Arc::downgrade(&self.active);
        let tag = notification.tag.clone();
        toast
            .Failed(
                &TypedEventHandler::<ToastNotification, ToastFailedEventArgs>::new(move |_, _| {
                    if let Some(active) = active.upgrade() {
                        remove_current(&active, &tag, generation);
                    }
                    Ok(())
                }),
            )
            .map_err(native_error)?;

        let previous = self
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&notification.tag);
        if let Some((_, toast)) = previous {
            self.notifier.Hide(&toast).map_err(native_error)?;
        }
        self.active
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(notification.tag.clone(), (generation, toast.clone()));
        if let Err(error) = self.notifier.Show(&toast) {
            remove_current(&self.active, &notification.tag, generation);
            return Err(native_error(error));
        }
        Ok(())
    }

    pub fn withdraw(&mut self, tag: &str) -> Result<(), NotificationError> {
        let previous = self
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(tag);
        let hidden = match previous {
            Some((_, toast)) => self.notifier.Hide(&toast),
            None => Ok(()),
        };
        let history = ToastNotificationManager::History().and_then(|history| {
            history.RemoveGroupedTagWithId(
                &HSTRING::from(native_tag(tag)),
                h!("aqua"),
                &self.identity,
            )
        });
        hidden.and(history).map_err(native_error)
    }
}

#[cfg(test)]
#[path = "../../tests/notifications/native.rs"]
mod tests;
