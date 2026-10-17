//! Hardware sleep, ported from slot.advance's tests: what closing the lid does once the game is
//! saved, on a scripted stand-in for the RG SP, and the parts omega-zero adds (Bluetooth, the
//! display put back as the lid opens, the power screen taken away when a shutdown is
//! cancelled).

use crate::common;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{tmp_root_with_carts, StubSnapshot, CLOCK_IS_SET};
use omega_input::{Action, Millis};
use omega_power::{Battery, Charge, LedState, Platform, Power, Woke};
use omega_store::{
    read_app_state, write_app_state, AppState, Core, Platform as CartPlatform, StateRing,
};
use omega_zero::app::{App, Phase};
use omega_zero::sleep::{
    BATTERY_FLOOR, LOW_BATTERY_SLEEP_S, MAX_WAKES, POWER_QUIET_MS, REST_AFTER_WAKE_MS,
    SECS_PER_PERCENT, SETTLE_MS,
};

/// How one suspend should go.
#[derive(Clone, Copy, Debug)]
enum Wake {
    /// The rtc1 alarm: asleep for the whole time asked.
    Alarm,
    /// Something else after this many seconds, with the lid still shut.
    Stray(f64),
    /// The lid opened after this many seconds.
    Lid(f64),
    /// The kernel would not suspend.
    Refused,
}

struct Rig {
    uptime: f64,
    lid_open: bool,
    percent: u8,
    charge: Charge,
    script: VecDeque<Wake>,
    asked: Vec<u32>,
    backlight: u8,
    displays_resumed: u32,
}

#[derive(Clone)]
struct Device(Arc<Mutex<Rig>>);

impl Device {
    fn new() -> Device {
        Device(Arc::new(Mutex::new(Rig {
            uptime: 100.0,
            lid_open: true,
            percent: 80,
            charge: Charge::Discharging,
            script: VecDeque::new(),
            asked: Vec::new(),
            backlight: 0,
            displays_resumed: 0,
        })))
    }

    fn rig(&self) -> std::sync::MutexGuard<'_, Rig> {
        self.0.lock().unwrap()
    }

    fn then(&self, wake: Wake) -> &Self {
        self.rig().script.push_back(wake);
        self
    }

    fn asked(&self) -> Vec<u32> {
        self.rig().asked.clone()
    }
}

struct SleepPlatform {
    device: Device,
    root: PathBuf,
}

impl Platform for SleepPlatform {
    fn set_backlight(&mut self, step: u8) {
        self.device.rig().backlight = step;
    }

    fn battery(&self) -> Option<Battery> {
        let r = self.device.rig();
        Some(Battery {
            percent: r.percent,
            charge: r.charge,
        })
    }

    fn charge(&self) -> Charge {
        self.device.rig().charge
    }

    fn set_led(&mut self, _state: LedState) {}

    fn poweroff(&mut self) -> ! {
        panic!("the tests check powering_off, they never power off")
    }

    fn restart(&mut self) -> ! {
        panic!("no restarts here")
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn now(&self) -> i64 {
        CLOCK_IS_SET
    }

    fn set_clock(&mut self, _secs: i64) {}

    fn set_rumble(&mut self, _strength: u16) {}

    fn resume_display(&mut self) {
        self.device.rig().displays_resumed += 1;
    }

    fn sleep_ready(&self) -> bool {
        true
    }

    fn suspend(&mut self, secs: u32) -> Woke {
        let mut r = self.device.rig();
        r.asked.push(secs);
        let wake = r.script.pop_front().unwrap_or(Wake::Alarm);
        match wake {
            Wake::Alarm => {
                r.uptime += f64::from(secs);
                Woke::slept(f64::from(secs))
            }
            Wake::Stray(after) => {
                r.uptime += after;
                Woke::slept(after)
            }
            Wake::Lid(after) => {
                r.uptime += after;
                r.lid_open = true;
                Woke::slept(after)
            }
            Wake::Refused => Woke::refused("Device or resource busy"),
        }
    }

    fn uptime(&self) -> Option<f64> {
        Some(self.device.rig().uptime)
    }

    fn lid_open(&self) -> Option<bool> {
        Some(self.device.rig().lid_open)
    }
}

/// omega-zero's doze timeout, frontend.rs's DOZE_TIMEOUT: software standby powers off after it.
const STANDBY_TIMEOUT: Duration = Duration::from_secs(180);

struct Test {
    _dir: tempfile::TempDir,
    root: PathBuf,
    app: App,
    device: Device,
    now: Millis,
}

fn playing() -> Test {
    playing_with(AppState::default())
}

/// Emerald playing, booted from `state` with it in the slot.
fn playing_with(state: AppState) -> Test {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    write_app_state(
        d.path(),
        &AppState {
            cart: Some("Emerald".into()),
            clock_set: true,
            utc_offset_min: 0,
            ..state
        },
    )
    .unwrap();
    let mut app = App::boot(d.path());
    app.set_snapshot(StubSnapshot::boxed());
    app.on_core_ready();
    for _ in 0..120 {
        app.update(1.0 / 60.0);
    }
    let device = Device::new();
    let platform = SleepPlatform {
        device: device.clone(),
        root: d.path().to_path_buf(),
    };
    app.set_power(Power::new(Box::new(platform), STANDBY_TIMEOUT));
    let root = d.path().to_path_buf();
    let now = app.now();
    Test {
        _dir: d,
        root,
        app,
        device,
        now,
    }
}

impl Test {
    fn wait(&mut self, ms: Millis) {
        let end = self.now + ms;
        while self.now < end {
            self.now = (self.now + 50).min(end);
            self.app.tick_ms(self.now);
        }
    }

    fn close_lid(&mut self) {
        self.device.rig().lid_open = false;
        self.app.apply(Action::LidClose);
    }

    fn open_lid(&mut self) {
        self.device.rig().lid_open = true;
        self.app.apply(Action::LidOpen);
    }

    /// What the device loop does: wait out the settle, then sleep once.
    fn sleep_once(&mut self) -> bool {
        self.wait(SETTLE_MS.max(REST_AFTER_WAKE_MS) + 50);
        assert!(
            self.app.sleep_due(),
            "not ready to sleep: {:?}",
            self.app.phase()
        );
        self.app.sleep_now()
    }

    fn dozing(&self) -> bool {
        matches!(self.app.phase(), Phase::Doze { .. })
    }

    fn playing(&self) -> bool {
        matches!(self.app.phase(), Phase::Playing { .. })
    }

    fn resume_on_card(&self) -> bool {
        StateRing::new(&self.root, CartPlatform::Gba, Core::Mgba, "Emerald")
            .read_resume()
            .is_ok_and(|r| r.is_some())
    }
}

// --- Closing and opening the lid ------------------------------------------------------

#[test]
fn closing_the_lid_saves_first_and_only_then_sleeps() {
    let mut t = playing();
    t.close_lid();
    assert!(
        t.resume_on_card(),
        "the game was not on the card before sleeping"
    );
    assert!(t.dozing());
    assert!(
        !t.app.sleep_due(),
        "slept before the settle: a flick of the lid would reach the kernel"
    );
    t.wait(SETTLE_MS + 50);
    assert!(t.app.sleep_due());
}

#[test]
fn a_quick_flick_of_the_lid_never_suspends() {
    let mut t = playing();
    t.close_lid();
    t.wait(SETTLE_MS / 2);
    t.open_lid();
    assert!(t.playing());
    t.wait(2_000);
    assert!(!t.app.sleep_due());
    assert!(t.device.asked().is_empty(), "the kernel was asked to sleep");
}

#[test]
fn opening_the_lid_carries_on_the_same_game_straight_away() {
    let mut t = playing();
    t.close_lid();
    t.device.then(Wake::Lid(600.0));
    assert!(t.sleep_once(), "the kernel's suspend was not reported");
    assert!(t.playing(), "{:?}", t.app.phase());
    assert_eq!(t.app.selected_stem(), Some("Emerald"));
    assert!(!t.app.powering_off());
}

#[test]
fn the_display_is_put_back_as_the_lid_opens_after_a_suspend() {
    let mut t = playing();
    t.close_lid();
    t.device.then(Wake::Stray(60.0)).then(Wake::Lid(60.0));
    t.sleep_once();
    assert_eq!(
        t.device.rig().displays_resumed,
        0,
        "a wake that went back to sleep lit the display"
    );
    t.sleep_once();
    assert!(t.playing());
    assert_eq!(
        t.device.rig().displays_resumed,
        1,
        "the 120 Hz registers were not set again on the wake"
    );
}

#[test]
fn the_lid_opening_mid_entry_still_resumes_even_if_the_suspend_failed() {
    let mut t = playing();
    t.close_lid();
    {
        let mut r = t.device.rig();
        r.script.push_back(Wake::Refused);
        r.lid_open = true;
    }
    t.wait(SETTLE_MS + 50);
    assert!(!t.app.sleep_now(), "a refused suspend counted as one");
    assert!(t.playing());
}

#[test]
fn many_sleep_and_wake_cycles_each_resume() {
    let mut t = playing();
    for _ in 0..30 {
        t.close_lid();
        t.device.then(Wake::Lid(5.0));
        t.sleep_once();
        assert!(t.playing());
        t.wait(POWER_QUIET_MS);
    }
    assert_eq!(t.device.asked().len(), 30);
}

// --- The sleep duration -------------------------------------------------------------------

#[test]
fn the_timer_with_the_lid_shut_shuts_down_with_the_game_still_seated() {
    let mut t = playing();
    t.close_lid();
    t.device.then(Wake::Alarm);
    t.sleep_once();
    assert!(t.app.powering_off(), "the sleep ran out without a shutdown");
    assert_eq!(
        read_app_state(&t.root).cart,
        Some("Emerald".into()),
        "a shutdown is not an eject: the next boot must resume the game"
    );
}

#[test]
fn the_default_sleep_is_thirty_minutes_from_the_lid_closing() {
    let mut t = playing();
    t.close_lid();
    t.sleep_once();
    assert_eq!(t.device.asked(), vec![30 * 60]);
}

#[test]
fn the_duration_counts_every_wake_in_between() {
    let mut t = playing();
    t.close_lid();
    t.device.then(Wake::Stray(600.0)).then(Wake::Stray(300.0));
    t.sleep_once();
    t.sleep_once();
    t.sleep_once();
    let asked = t.device.asked();
    assert_eq!(asked[0], 1800);
    assert!((1199..=1200).contains(&asked[1]), "{asked:?}");
    assert!((899..=900).contains(&asked[2]), "{asked:?}");
    assert!(t.app.powering_off());
}

#[test]
fn the_sleep_setting_is_used() {
    for minutes in omega_store::SLEEP_MINUTES {
        let mut t = playing_with(AppState {
            sleep_minutes: minutes,
            ..AppState::default()
        });
        t.close_lid();
        t.sleep_once();
        assert_eq!(t.device.asked(), vec![u32::from(minutes) * 60]);
    }
}

#[test]
fn opening_the_lid_as_the_sleep_runs_out_keeps_the_game() {
    let mut t = playing();
    t.close_lid();
    t.device.then(Wake::Alarm);
    t.sleep_once();
    assert!(t.app.powering_off());
    assert!(t.app.power_screen().is_some(), "no power off screen");
    t.open_lid();
    assert!(
        !t.app.powering_off(),
        "the shutdown went ahead with the lid open"
    );
    assert!(t.playing(), "{:?}", t.app.phase());
    assert_eq!(
        t.app.power_screen(),
        None,
        "the power off screen stayed over the game"
    );
}

// --- POWER and other wakes with the lid shut ------------------------------------------------

#[test]
fn power_with_the_lid_shut_never_lights_the_screen() {
    let mut t = playing();
    t.close_lid();
    for action in [
        Action::PowerPress,
        Action::PowerTap,
        Action::PowerHold,
        Action::PowerOff,
    ] {
        t.app.apply(action);
        assert!(t.dozing(), "{action:?} woke it with the lid shut");
        assert_eq!(t.device.rig().backlight, 0);
    }
    t.device.then(Wake::Stray(30.0));
    t.sleep_once();
    t.app.apply(Action::PowerPress);
    t.app.apply(Action::PowerTap);
    assert!(t.dozing());
    assert!(!t.app.powering_off());
    t.sleep_once();
    assert_eq!(t.device.asked().len(), 2, "it did not go back to sleep");
}

#[test]
fn the_press_that_woke_it_cannot_also_put_it_back_to_sleep() {
    let mut t = playing();
    t.close_lid();
    t.device.then(Wake::Lid(60.0));
    t.sleep_once();
    assert!(t.playing());
    t.app.apply(Action::PowerPress);
    t.wait(200);
    t.app.apply(Action::PowerTap);
    assert!(t.playing(), "a queued press sent it straight back to sleep");
    t.app.apply(Action::PowerHold);
    assert!(
        t.app.power_menu().is_none(),
        "a queued press opened the power menu"
    );
    t.wait(POWER_QUIET_MS);
    t.app.apply(Action::PowerPress);
    t.app.apply(Action::PowerTap);
    assert!(
        t.dozing(),
        "after the quiet, a tap of POWER sleeps as before"
    );
}

#[test]
fn a_wake_storm_falls_back_to_software_standby() {
    let mut t = playing();
    t.app.apply(Action::LidClose);
    t.device.rig().lid_open = false;
    for _ in 0..=MAX_WAKES {
        t.device.then(Wake::Stray(1.0));
    }
    for _ in 0..=MAX_WAKES {
        t.sleep_once();
    }
    assert!(!t.app.sleep_due(), "still trying to sleep after a storm");
    assert!(t.dozing());
    t.wait(STANDBY_TIMEOUT.as_millis() as Millis + 100);
    assert!(
        t.app.powering_off(),
        "software standby did not time out as before"
    );
}

#[test]
fn a_failed_suspend_is_retried_once_then_falls_back() {
    let mut t = playing();
    t.close_lid();
    t.device.then(Wake::Refused).then(Wake::Refused);
    t.sleep_once();
    assert!(t.dozing());
    t.sleep_once();
    assert!(!t.app.sleep_due(), "kept trying a sleep that fails");
    assert!(t.dozing());
    assert!(!t.app.powering_off());
    t.wait(STANDBY_TIMEOUT.as_millis() as Millis + 100);
    assert!(t.app.powering_off());
}

#[test]
fn a_failed_suspend_that_recovers_carries_on_sleeping() {
    let mut t = playing();
    t.close_lid();
    t.device.then(Wake::Refused).then(Wake::Lid(10.0));
    t.sleep_once();
    t.sleep_once();
    assert!(t.playing());
}

#[test]
fn hardware_sleep_never_times_out_as_software_standby_does() {
    let mut t = playing();
    t.close_lid();
    t.wait(STANDBY_TIMEOUT.as_millis() as Millis * 2);
    assert!(
        !t.app.powering_off(),
        "the three-minute standby timeout shut down a doze that sleeps in hardware"
    );
    assert!(t.app.sleep_due());
}

// --- Battery -----------------------------------------------------------------------------------

#[test]
fn a_low_battery_wakes_in_time_to_leave_ten_percent() {
    let mut t = playing_with(AppState {
        sleep_minutes: 480,
        ..AppState::default()
    });
    t.device.rig().percent = 20;
    t.close_lid();
    t.sleep_once();
    let want = (f64::from(20 - BATTERY_FLOOR) * SECS_PER_PERCENT).ceil() as u32;
    assert_eq!(t.device.asked(), vec![want]);
    assert!(
        t.app.powering_off(),
        "it did not shut down at the battery deadline"
    );
}

#[test]
fn at_the_floor_it_sleeps_only_briefly() {
    let mut t = playing();
    t.device.rig().percent = 9;
    t.close_lid();
    t.sleep_once();
    assert_eq!(t.device.asked(), vec![LOW_BATTERY_SLEEP_S as u32]);
}

#[test]
fn charging_while_asleep_lifts_the_battery_cap() {
    let mut t = playing();
    {
        let mut r = t.device.rig();
        r.percent = 12;
        r.charge = Charge::Charging;
    }
    t.close_lid();
    t.sleep_once();
    assert_eq!(t.device.asked(), vec![30 * 60]);
}

#[test]
fn a_critical_battery_shuts_down_instead_of_sleeping() {
    let mut t = playing();
    t.device.rig().percent = 5;
    t.close_lid();
    t.wait(SETTLE_MS + 50);
    t.app.sleep_now();
    assert!(t.app.powering_off());
    assert!(t.device.asked().is_empty(), "slept on a critical battery");
}

// --- Never lose the game -------------------------------------------------------------------------

fn block_saves(root: &Path) {
    let dir = root
        .join("States")
        .join(CartPlatform::Gba.dir_name())
        .join(Core::Mgba.as_str())
        .join("Emerald");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.parent().unwrap()).unwrap();
    std::fs::write(&dir, b"not a directory").unwrap();
}

fn unblock_saves(root: &Path) {
    let dir = root
        .join("States")
        .join(CartPlatform::Gba.dir_name())
        .join(Core::Mgba.as_str())
        .join("Emerald");
    std::fs::remove_file(dir).unwrap();
}

#[test]
fn an_unsaved_game_is_never_shut_down_by_the_timer() {
    let mut t = playing();
    block_saves(&t.root);
    t.close_lid();
    assert!(!t.resume_on_card());
    t.device.then(Wake::Alarm).then(Wake::Alarm);
    t.sleep_once();
    assert!(
        !t.app.powering_off(),
        "shut down with the only copy of the game in RAM"
    );
    assert!(t.dozing());
    t.sleep_once();
    assert!(!t.app.powering_off());
    assert_eq!(
        t.device.asked()[1],
        30 * 60,
        "an unsaved game did not sleep another 30 minutes"
    );
    unblock_saves(&t.root);
    t.device.then(Wake::Alarm);
    t.sleep_once();
    assert!(t.resume_on_card(), "the save was not retried");
    assert!(
        t.app.powering_off(),
        "once saved, the timer shuts down as normal"
    );
}

/// A core that only gives up its state once `give` is set, and reports whether a game is
/// running at all.
struct SilentCore {
    give: Arc<std::sync::atomic::AtomicBool>,
    running: bool,
}

impl omega_zero::persist::Snapshot for SilentCore {
    fn state(&self) -> Option<Vec<u8>> {
        self.give
            .load(std::sync::atomic::Ordering::Relaxed)
            .then(|| b"state".to_vec())
    }

    fn save_ram(&self) -> Option<Vec<u8>> {
        None
    }

    fn thumb(&self) -> Option<Vec<u8>> {
        None
    }

    fn load(&self, _state: Vec<u8>) {}

    fn running(&self) -> bool {
        self.running
    }
}

fn silent_core(t: &mut Test, running: bool) -> Arc<std::sync::atomic::AtomicBool> {
    let give = Arc::new(std::sync::atomic::AtomicBool::new(false));
    t.app.set_snapshot(Box::new(SilentCore {
        give: give.clone(),
        running,
    }));
    give
}

#[test]
fn a_running_game_whose_core_gives_no_state_is_never_shut_down_by_the_timer() {
    let mut t = playing();
    let give = silent_core(&mut t, true);
    t.close_lid();
    assert!(!t.resume_on_card());
    t.device.then(Wake::Alarm);
    t.sleep_once();
    assert!(
        !t.app.powering_off(),
        "treated a game the core could not save as saved"
    );
    assert!(t.dozing());
    give.store(true, std::sync::atomic::Ordering::Relaxed);
    t.device.then(Wake::Alarm);
    t.sleep_once();
    assert!(t.resume_on_card(), "the save was not retried");
    assert!(t.app.powering_off(), "once saved, the timer shuts down");
}

#[test]
fn a_core_still_loading_has_nothing_to_save_and_the_timer_works_as_normal() {
    let mut t = playing();
    silent_core(&mut t, false);
    t.close_lid();
    t.device.then(Wake::Alarm);
    t.sleep_once();
    assert!(t.app.powering_off());
}

#[test]
fn an_unsaved_game_still_resumes_when_the_lid_opens() {
    let mut t = playing();
    block_saves(&t.root);
    t.close_lid();
    t.device.then(Wake::Lid(60.0));
    t.sleep_once();
    assert!(t.playing());
}

#[test]
fn software_standby_never_shuts_down_an_unsaved_game_either() {
    let mut t = playing();
    block_saves(&t.root);
    t.close_lid();
    t.device.then(Wake::Refused).then(Wake::Refused);
    t.sleep_once();
    t.sleep_once();
    t.wait(STANDBY_TIMEOUT.as_millis() as Millis * 2);
    assert!(!t.app.powering_off());
    unblock_saves(&t.root);
    t.wait(STANDBY_TIMEOUT.as_millis() as Millis + 100);
    assert!(t.app.powering_off());
    assert!(t.resume_on_card());
}

// --- Bluetooth ---------------------------------------------------------------------------------------

#[test]
fn with_bluetooth_on_a_doze_is_software_standby() {
    let mut t = playing_with(AppState {
        bluetooth: true,
        ..AppState::default()
    });
    t.close_lid();
    assert!(t.resume_on_card());
    t.wait(SETTLE_MS + REST_AFTER_WAKE_MS);
    assert!(!t.app.sleep_due(), "asked to suspend with Bluetooth on");
    t.wait(STANDBY_TIMEOUT.as_millis() as Millis + 100);
    assert!(t.app.powering_off(), "the standby timeout no longer works");
    assert!(t.device.asked().is_empty());
}

// --- Platforms without hardware sleep ---------------------------------------------------------------

#[test]
fn without_hardware_sleep_the_doze_is_software_standby() {
    let d = tmp_root_with_carts(&["Emerald"]);
    let mut a = common::app_playing_in(d.path(), "Emerald");
    a.set_power(common::panel(d.path(), Duration::from_secs(2)).0);
    a.apply(Action::LidClose);
    for _ in 0..200 {
        a.update(1.0 / 60.0);
        assert!(!a.sleep_due(), "asked to suspend a platform that cannot");
    }
    assert!(a.powering_off(), "the doze timeout no longer works");
}
