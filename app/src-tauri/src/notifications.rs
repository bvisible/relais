//! macOS notifications, through the system's own notification center. A button
//! on them opens the panel on the confirmation of a switch: that answer comes
//! back to the delegate here, whatever the app is doing.

use std::sync::OnceLock;

use block2::{Block, RcBlock};
use objc2::rc::Retained;
use objc2::runtime::{Bool, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, AnyThread};
use objc2_foundation::{NSArray, NSError, NSSet, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotification, UNNotificationAction,
    UNNotificationActionOptions, UNNotificationCategory, UNNotificationCategoryOptions,
    UNNotificationDismissActionIdentifier, UNNotificationPresentationOptions,
    UNNotificationRequest, UNNotificationResponse, UNNotificationSound, UNUserNotificationCenter,
    UNUserNotificationCenterDelegate,
};
use tauri::{AppHandle, Emitter};

use crate::{platform, tray};

/// What the panel listens to: the account to confirm a switch to.
pub const SWITCH_TO_EVENT: &str = "relais-switch-to";
const ACTION: &str = "switch";

static APP: OnceLock<AppHandle> = OnceLock::new();

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "RelaisNotificationDelegate"]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl UNUserNotificationCenterDelegate for Delegate {
        // Relais counts as the app in front while its panel is open.
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            handler: &Block<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            handler.call((UNNotificationPresentationOptions::Banner
                | UNNotificationPresentationOptions::List
                | UNNotificationPresentationOptions::Sound,));
        }

        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            handler: &Block<dyn Fn()>,
        ) {
            let dismissed = unsafe { UNNotificationDismissActionIdentifier };
            if !response.actionIdentifier().isEqualToString(dismissed) {
                let target = response
                    .notification()
                    .request()
                    .content()
                    .categoryIdentifier()
                    .to_string();
                if let Some(app) = APP.get() {
                    tray::open_panel(app);
                    if !target.is_empty() {
                        let _ = app.emit(SWITCH_TO_EVENT, target);
                    }
                }
            }
            handler.call(());
        }
    }
);

impl Delegate {
    fn new() -> Retained<Self> {
        let this = Self::alloc().set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

/// Takes the answers to notifications, and asks once for leave to show them.
/// `aliases` are the accounts a notification can offer to switch to.
pub fn prepare(app: AppHandle, aliases: &[String]) {
    let _ = APP.set(app);
    let center = UNUserNotificationCenter::currentNotificationCenter();
    // The center keeps a weak reference: the delegate lives as long as the app.
    let delegate: &'static Delegate = Box::leak(Box::new(Delegate::new()));
    center.setDelegate(Some(ProtocolObject::from_ref(delegate)));
    let categories: Vec<Retained<UNNotificationCategory>> =
        aliases.iter().map(|alias| category(alias)).collect();
    let references: Vec<&UNNotificationCategory> = categories.iter().map(|c| &**c).collect();
    center.setNotificationCategories(&NSSet::from_slice(&references));
    let asked = RcBlock::new(|_granted: Bool, _error: *mut NSError| {});
    center.requestAuthorizationWithOptions_completionHandler(
        UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
        &asked,
    );
}

/// The kind of notification that offers to switch to `alias`.
fn category(alias: &str) -> Retained<UNNotificationCategory> {
    let title = platform::text("alert.switch", &[("alias", alias)]);
    let action = UNNotificationAction::actionWithIdentifier_title_options(
        &NSString::from_str(ACTION),
        &NSString::from_str(&title),
        UNNotificationActionOptions::Foreground,
    );
    UNNotificationCategory::categoryWithIdentifier_actions_intentIdentifiers_options(
        &NSString::from_str(alias),
        &NSArray::from_retained_slice(&[action]),
        &NSArray::new(),
        UNNotificationCategoryOptions::empty(),
    )
}

/// Shows a notification. With `target`, it offers to switch to that account.
pub fn show(title: &str, body: &str, target: Option<&str>) {
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(title));
    content.setBody(&NSString::from_str(body));
    content.setSound(Some(&UNNotificationSound::defaultSound()));
    if let Some(alias) = target {
        content.setCategoryIdentifier(&NSString::from_str(alias));
    }
    let id = format!("relais-{}", chrono::Utc::now().timestamp_millis());
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
        &NSString::from_str(&id),
        &content,
        None,
    );
    // Fail open: a notification that cannot show changes nothing else.
    UNUserNotificationCenter::currentNotificationCenter()
        .addNotificationRequest_withCompletionHandler(&request, None);
}
