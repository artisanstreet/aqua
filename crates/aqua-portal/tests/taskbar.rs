#![cfg(target_os = "linux")]

mod support;

use std::collections::HashMap;

use aqua_core::Oklch;
use aqua_portal::taskbar::{BadgeError, Launcher, TaskbarBadge};
use aqua_shell::AppId;
use futures_lite::StreamExt;
use zbus::zvariant::OwnedValue;

fn badge(count: u32) -> TaskbarBadge {
    TaskbarBadge {
        count,
        color: Oklch {
            l: 0.6,
            c: 0.2,
            h: 30.0,
            alpha: 1.0,
        },
    }
}

#[test]
fn a_badge_count_emits_a_launcher_entry_update_for_the_app_id() {
    let Some(bus) = support::bus() else { return };
    let dock = zbus::block_on(
        zbus::connection::Builder::address(bus.address())
            .unwrap()
            .build(),
    )
    .unwrap();
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface("com.canonical.Unity.LauncherEntry")
        .unwrap()
        .member("Update")
        .unwrap()
        .build();
    let mut updates =
        zbus::block_on(zbus::MessageStream::for_match_rule(rule, &dock, None)).unwrap();
    let mut next = || {
        let message = zbus::block_on(futures_lite::future::or(
            async { updates.next().await.map(|message| message.unwrap()) },
            async {
                async_io::Timer::after(support::WAIT).await;
                None
            },
        ))
        .expect("the dock receives an Update signal");
        let (uri, properties) = message
            .body()
            .deserialize::<(String, HashMap<String, OwnedValue>)>()
            .unwrap();
        let count = i64::try_from(&properties["count"]).unwrap();
        let visible = bool::try_from(&properties["count-visible"]).unwrap();
        (uri, count, visible)
    };

    let launcher = Launcher::on(
        bus.bus(),
        Some(AppId::new("com.artisanstreet.Editor").unwrap()),
    );
    launcher.set_badge(Some(badge(12))).unwrap();
    assert_eq!(
        next(),
        (
            "application://com.artisanstreet.Editor.desktop".into(),
            12,
            true
        )
    );
    launcher.set_badge(None).unwrap();
    let (_, count, visible) = next();
    assert_eq!((count, visible), (0, false));
}

#[test]
fn badges_without_an_app_id_report_unsupported() {
    let Some(bus) = support::bus() else { return };
    let launcher = Launcher::on(bus.bus(), None);
    assert_eq!(
        launcher.set_badge(Some(badge(3))),
        Err(BadgeError::Unsupported)
    );
}
