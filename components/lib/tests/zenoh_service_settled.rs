//! A crate that depends on `slime-components` can name the result of `Core::settle`, so a service
//! component can drive the documented state machine.

use slime_components::zenoh_service::Settled;

#[test]
fn a_user_of_the_crate_can_name_and_match_every_settlement() {
    let all = [
        Settled::Idle,
        Settled::Waiting,
        Settled::Declared { id: 1 },
        Settled::Published { reached: 2 },
    ];
    let names: Vec<&str> = all
        .iter()
        .map(|settled| match settled {
            Settled::Idle => "idle",
            Settled::Waiting => "waiting",
            Settled::Declared { .. } => "declared",
            Settled::Published { .. } => "published",
        })
        .collect();
    assert_eq!(names, ["idle", "waiting", "declared", "published"]);
}
