use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{sync_channel, RecvTimeoutError, TrySendError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use block2::{DynBlock, RcBlock};
use objc2::rc::Retained;
use objc2::runtime::{Bool, ProtocolObject};
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass};
use objc2_foundation::{NSArray, NSBundle, NSError, NSObject, NSObjectProtocol, NSSet, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNErrorCode, UNMutableNotificationContent, UNNotification,
    UNNotificationAction, UNNotificationActionOptions, UNNotificationCategory,
    UNNotificationCategoryOptions, UNNotificationDefaultActionIdentifier,
    UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationResponse,
    UNUserNotificationCenter, UNUserNotificationCenterDelegate,
};

pub use aqua_shell::notifications::{
    native_tag, AppIdentity, Notification, NotificationAction, NotificationError,
    NotificationResponse, ResponseHandler,
};

pub mod activation {
    pub use aqua_shell::notifications::activation::{
        activation_url, parse_activation_url, Activation, ActivationUrlError, Launch,
        ACTIVATION_FLAG, SCHEME,
    };

    pub fn handle_launch() -> Launch {
        Launch::Ordinary
    }
}

const CALL_TIMEOUT: Duration = Duration::from_secs(3);
const ACTIVE_LIMIT: usize = 256;
const ACTION_PREFIX: &str = "action:";
const CATEGORY_PREFIX: &str = "aqua.actions.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Authorization {
    Pending,
    Granted,
    Denied,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Bundle {
    App { identifier: String },
    Missing,
}

impl Bundle {
    pub fn of_this_process() -> Bundle {
        let main = NSBundle::mainBundle();
        let path = main.bundlePath().to_string();
        match main.bundleIdentifier() {
            Some(identifier) if path.ends_with(".app") => Bundle::App {
                identifier: identifier.to_string(),
            },
            _ => Bundle::Missing,
        }
    }
}

pub fn response_of(
    tag: String,
    action: &str,
    default_action: &str,
) -> Option<NotificationResponse> {
    match action == default_action {
        true => Some(NotificationResponse {
            tag,
            action_id: None,
        }),
        false => action
            .strip_prefix(ACTION_PREFIX)
            .map(|id| NotificationResponse {
                tag,
                action_id: Some(id.to_owned()),
            }),
    }
}

pub fn category_identifier(actions: &[NotificationAction]) -> String {
    let joined: String = actions
        .iter()
        .flat_map(|action| {
            [
                action.id.as_str(),
                "\u{1f}",
                action.label.as_str(),
                "\u{1e}",
            ]
        })
        .collect();
    format!("{CATEGORY_PREFIX}{}", native_tag(&joined))
}

type Active = Arc<Mutex<HashSet<String>>>;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

struct Gate {
    answer: Mutex<Authorization>,
    settled: Condvar,
}

impl Gate {
    fn settle(&self, answer: Authorization) {
        *lock(&self.answer) = answer;
        self.settled.notify_all();
    }
}

fn answer_of(granted: bool, error: Option<&NSError>) -> Authorization {
    match (granted, error) {
        (true, _) => Authorization::Granted,
        (false, Some(error)) if error.code() == UNErrorCode::NotificationsNotAllowed.0 => {
            Authorization::Denied
        }
        (false, Some(error)) => Authorization::Failed(error.localizedDescription().to_string()),
        (false, None) => Authorization::Denied,
    }
}

struct DelegateState {
    response: ResponseHandler,
    active: Active,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "AquaNotificationDelegate"]
    #[ivars = DelegateState]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl UNUserNotificationCenterDelegate for Delegate {
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            done: &DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            done.call((
                UNNotificationPresentationOptions::Banner | UNNotificationPresentationOptions::List,
            ));
        }

        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            done: &DynBlock<dyn Fn()>,
        ) {
            let tag = response.notification().request().identifier().to_string();
            let action = response.actionIdentifier().to_string();
            let default_action = unsafe { UNNotificationDefaultActionIdentifier }.to_string();
            if let Some(answer) = response_of(tag, &action, &default_action) {
                lock(&self.ivars().active).remove(&answer.tag);
                (self.ivars().response)(answer);
            }
            done.call(());
        }
    }
);

impl Delegate {
    fn new(state: DelegateState) -> Retained<Delegate> {
        let this = Self::alloc().set_ivars(state);
        unsafe { msg_send![super(this), init] }
    }
}

pub struct NotificationCenter {
    center: Retained<UNUserNotificationCenter>,
    _delegate: Retained<Delegate>,
    gate: Arc<Gate>,
    categories: HashMap<String, Retained<UNNotificationCategory>>,
    active: Active,
}

fn native(reason: impl std::fmt::Display) -> NotificationError {
    NotificationError::Native(reason.to_string())
}

impl NotificationCenter {
    pub fn new(
        _identity: AppIdentity,
        response: ResponseHandler,
    ) -> Result<Self, NotificationError> {
        if Bundle::of_this_process() == Bundle::Missing {
            return Err(NotificationError::Unsupported);
        }
        let center = UNUserNotificationCenter::currentNotificationCenter();
        let active: Active = Arc::default();
        let delegate = Delegate::new(DelegateState {
            response,
            active: active.clone(),
        });
        center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        let gate = Arc::new(Gate {
            answer: Mutex::new(Authorization::Pending),
            settled: Condvar::new(),
        });
        let settle = gate.clone();
        let answered = RcBlock::new(move |granted: Bool, error: *mut NSError| {
            settle.settle(answer_of(granted.as_bool(), unsafe { error.as_ref() }));
        });
        center.requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
            &answered,
        );
        Ok(Self {
            center,
            _delegate: delegate,
            gate,
            categories: HashMap::new(),
            active,
        })
    }

    pub fn authorization(&self) -> Authorization {
        lock(&self.gate.answer).clone()
    }

    pub fn wait_for_authorization(&self, limit: Duration) -> Authorization {
        let deadline = Instant::now() + limit;
        let mut answer = lock(&self.gate.answer);
        while *answer == Authorization::Pending {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                break;
            };
            answer = self
                .gate
                .settled
                .wait_timeout(answer, left)
                .unwrap_or_else(|error| error.into_inner())
                .0;
        }
        answer.clone()
    }

    fn category(&mut self, actions: &[NotificationAction]) -> Option<Retained<NSString>> {
        if actions.is_empty() {
            return None;
        }
        let identifier = category_identifier(actions);
        if !self.categories.contains_key(&identifier) {
            let buttons: Vec<Retained<UNNotificationAction>> = actions
                .iter()
                .map(|action| {
                    UNNotificationAction::actionWithIdentifier_title_options(
                        &NSString::from_str(&format!("{ACTION_PREFIX}{}", action.id)),
                        &NSString::from_str(&action.label),
                        UNNotificationActionOptions::Foreground,
                    )
                })
                .collect();
            let category =
                UNNotificationCategory::categoryWithIdentifier_actions_intentIdentifiers_options(
                    &NSString::from_str(&identifier),
                    &NSArray::from_retained_slice(&buttons),
                    &NSArray::new(),
                    UNNotificationCategoryOptions::empty(),
                );
            self.categories.insert(identifier.clone(), category);
            let all: Vec<Retained<UNNotificationCategory>> =
                self.categories.values().cloned().collect();
            self.center
                .setNotificationCategories(&NSSet::from_retained_slice(&all));
        }
        Some(NSString::from_str(&identifier))
    }

    pub fn show(&mut self, notification: Notification) -> Result<(), NotificationError> {
        notification.validate()?;
        match self.authorization() {
            Authorization::Denied => return Err(native("macOS denied notifications for this app")),
            Authorization::Failed(reason) => return Err(native(reason)),
            Authorization::Pending | Authorization::Granted => {}
        }
        {
            let active = lock(&self.active);
            if active.len() >= ACTIVE_LIMIT && !active.contains(&notification.tag) {
                return Err(NotificationError::TooManyActive);
            }
        }
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(&notification.title));
        content.setBody(&NSString::from_str(&notification.body));
        if let Some(category) = self.category(&notification.actions) {
            content.setCategoryIdentifier(&category);
        }
        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
            &NSString::from_str(&notification.tag),
            &content,
            None,
        );
        let (sender, added) = sync_channel(1);
        let completion = RcBlock::new(move |error: *mut NSError| {
            let failure =
                unsafe { error.as_ref() }.map(|error| error.localizedDescription().to_string());
            match sender.try_send(failure) {
                Ok(()) | Err(TrySendError::Disconnected(_)) => {}
                Err(TrySendError::Full(_)) => {
                    unreachable!("invariant: a notification request completes once")
                }
            }
        });
        self.center
            .addNotificationRequest_withCompletionHandler(&request, Some(&completion));
        match added.recv_timeout(CALL_TIMEOUT) {
            Ok(None) => {
                lock(&self.active).insert(notification.tag);
                Ok(())
            }
            Ok(Some(reason)) => Err(native(reason)),
            Err(RecvTimeoutError::Timeout) => Err(native(
                "UNUserNotificationCenter did not confirm the request within 3 s",
            )),
            Err(RecvTimeoutError::Disconnected) => {
                Err(native("UNUserNotificationCenter dropped the request"))
            }
        }
    }

    pub fn withdraw(&mut self, tag: &str) -> Result<(), NotificationError> {
        lock(&self.active).remove(tag);
        let identifiers = NSArray::from_retained_slice(&[NSString::from_str(tag)]);
        self.center
            .removeDeliveredNotificationsWithIdentifiers(&identifiers);
        self.center
            .removePendingNotificationRequestsWithIdentifiers(&identifiers);
        Ok(())
    }
}
