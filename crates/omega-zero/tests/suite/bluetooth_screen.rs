//! The Bluetooth screen and the radio as the app knows it: Settings' BLUETOOTH row, switching
//! Bluetooth, the devices list and what each key does on each device, its notes, the looks for
//! devices X asks for, the state the card keeps and the top bar's icon.

use crate::common;

use std::sync::{Arc, Mutex};

use common::{app_booting_at, tmp_root_with_carts, CLOCK_IS_SET};
use omega_input::{Action, Btn, Millis};
use omega_store::{read_app_state, write_app_state, AppState};
use omega_ui::network::networks_shown;
use omega_ui::SettingsRow;
use omega_zero::app::{App, Phase};
use omega_zero::bluetooth::{
    BluetoothJob, BluetoothJobs, BluetoothNews, Device, DeviceKind, Fail, Status, LOOK_MS,
    REFRESH_MS,
};
use omega_zero::bluetooth_screen::{DeviceLine, Lit, Sel, Summary, Watch, DOING_MS, NOTE_MS};
use omega_zero::nav::Dir;
use omega_zero::screens::bar::FORGET;
use omega_zero::screens::bar::SCAN;
use omega_zero::wifi_screen::SettingLine;
use tempfile::TempDir;

#[derive(Default)]
struct RadioState {
    absent: bool,
    asked: Vec<BluetoothJob>,
    news: Vec<BluetoothNews>,
}

/// A radio that answers only what the test says, and remembers what it was asked.
#[derive(Clone, Default)]
struct Radio(Arc<Mutex<RadioState>>);

impl Radio {
    fn absent() -> Self {
        let r = Radio::default();
        r.0.lock().unwrap().absent = true;
        r
    }

    fn asked(&self) -> Vec<BluetoothJob> {
        self.0.lock().unwrap().asked.clone()
    }

    fn forget_asked(&self) {
        self.0.lock().unwrap().asked.clear();
    }

    fn say(&self, news: BluetoothNews) {
        self.0.lock().unwrap().news.push(news);
    }
}

impl BluetoothJobs for Radio {
    fn present(&self) -> bool {
        !self.0.lock().unwrap().absent
    }

    fn ask(&mut self, job: BluetoothJob) {
        self.0.lock().unwrap().asked.push(job);
    }

    fn news(&mut self) -> Vec<BluetoothNews> {
        std::mem::take(&mut self.0.lock().unwrap().news)
    }
}

const PAD: &str = "E4:17:D8:9A:01:22";
const CANS: &str = "38:18:4C:AB:CD:EF";
const KEYS: &str = "DC:2C:26:01:02:03";
const BAND: &str = "C8:0F:10:4D:7A:25";
/// A device found that gave no name: a beacon, most likely.
const ODD: &str = "4C:4C:6B:1A:80:D3";

fn device(mac: &str, name: &str, kind: DeviceKind, paired: bool, connected: bool) -> Device {
    Device {
        mac: mac.into(),
        name: name.into(),
        kind,
        paired,
        trusted: paired,
        connected,
    }
}

/// flow.html's devices: the gamepad connected, the headphones paired, and the keyboard and the
/// band found.
fn flows() -> Vec<Device> {
    vec![
        device(PAD, "8BitDo SN30 Pro", DeviceKind::Gamepad, true, true),
        device(CANS, "WH-1000XM4", DeviceKind::Audio, true, false),
        device(KEYS, "Keychron K3", DeviceKind::Keyboard, false, false),
        device(BAND, "Mi Smart Band 4", DeviceKind::Other, false, false),
    ]
}

/// flow.html's paired devices alone.
fn paired() -> Vec<Device> {
    flows().into_iter().filter(|d| d.paired).collect()
}

fn status(devices: &[Device]) -> BluetoothNews {
    BluetoothNews::Status(Status {
        enabled: true,
        powered: true,
        discovering: true,
        devices: devices.to_vec(),
    })
}

fn done(job: BluetoothJob, result: Result<(), Fail>) -> BluetoothNews {
    BluetoothNews::Done { job, result }
}

fn pair(mac: &str) -> BluetoothJob {
    BluetoothJob::Pair(mac.into())
}

fn connect(mac: &str) -> BluetoothJob {
    BluetoothJob::Connect(mac.into())
}

struct Rig {
    d: TempDir,
    a: App,
    radio: Radio,
    now: Millis,
}

impl Rig {
    fn with(state: AppState, radio: Radio) -> Rig {
        let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
        write_app_state(
            d.path(),
            &AppState {
                clock_set: true,
                ..state
            },
        )
        .expect("write omega-zero.state");
        let (mut a, _) = app_booting_at(d.path(), CLOCK_IS_SET);
        a.set_bluetooth_jobs(Box::new(radio.clone()));
        Rig {
            d,
            a,
            radio,
            now: 1_000,
        }
    }

    fn new() -> Rig {
        Rig::with(AppState::default(), Radio::default())
    }

    fn on() -> Rig {
        Rig::with(
            AppState {
                bluetooth: true,
                ..AppState::default()
            },
            Radio::default(),
        )
    }

    fn press(&mut self, btn: Btn) {
        self.a.apply(Action::GbaDown(btn));
        self.a.apply(Action::GbaUp(btn));
    }

    fn tick(&mut self, ms: Millis) {
        self.now += ms;
        self.a.tick_ms(self.now);
    }

    fn open_menu_at(&mut self, row: SettingsRow) {
        if self.a.settings_row().is_none() {
            self.press(Btn::Start);
        }
        assert!(self.a.settings_row().is_some(), "START opened no Settings");
        for _ in SettingsRow::ALL {
            if self.a.settings_row() == Some(row) {
                return;
            }
            self.press(Btn::Down);
        }
        panic!("Settings has no {row:?} row");
    }

    fn open(&mut self) {
        self.open_menu_at(SettingsRow::Bluetooth);
        self.press(Btn::A);
        assert!(
            matches!(self.a.phase(), Phase::Bluetooth(_)),
            "A did not open Bluetooth: {:?}",
            self.a.phase()
        );
    }

    /// Opens the Bluetooth screen with Bluetooth on and these devices known, before any look.
    fn opened_with(&mut self, devices: &[Device]) {
        self.open();
        if !self.a.bluetooth().enabled() {
            self.press(Btn::A);
        }
        self.radio.say(status(devices));
        self.tick(16);
        self.radio.forget_asked();
    }

    /// Opens the Bluetooth screen with Bluetooth on, and looks once with X: the radio knows
    /// these devices, during the look and after it.
    fn on_with(&mut self, devices: &[Device]) {
        self.opened_with(devices);
        self.press(Btn::X);
        self.radio.say(status(devices));
        self.tick(16);
        self.tick(LOOK_MS);
        self.radio.say(status(devices));
        self.tick(16);
        assert!(!self.a.bluetooth().looking(), "the look did not end");
        self.radio.forget_asked();
    }

    /// The jobs for discovery the radio has been asked, on and off, in turn.
    fn scans(&self) -> Vec<bool> {
        self.radio
            .asked()
            .into_iter()
            .filter_map(|job| match job {
                BluetoothJob::Scan(on) => Some(on),
                _ => None,
            })
            .collect()
    }

    /// Ticks for `ms`, a status from the radio answering every refresh it is asked.
    fn answering(&mut self, devices: &[Device], ms: Millis) {
        let mut left = ms;
        while left > 0 {
            let step = left.min(500);
            if self.radio.asked().last() == Some(&BluetoothJob::Refresh) {
                self.radio.say(status(devices));
            }
            self.tick(step);
            left -= step;
        }
    }

    fn sel(&self) -> Sel {
        match self.a.phase() {
            Phase::Bluetooth(s) => s.sel.clone(),
            other => panic!("not on the Bluetooth screen: {other:?}"),
        }
    }

    fn go_to(&mut self, name: &str) {
        for _ in 0..40 {
            if self.lit_name().as_deref() == Some(name) {
                return;
            }
            self.press(Btn::Down);
        }
        panic!("{name} is not in the list");
    }

    fn view(&mut self) -> omega_zero::bluetooth_screen::BluetoothView {
        self.a
            .bluetooth_view()
            .expect("the Bluetooth modal is showing")
    }

    fn notes(&mut self) -> Vec<String> {
        self.view().notes
    }

    fn names(&mut self) -> Vec<String> {
        self.view().devices.into_iter().map(|d| d.name).collect()
    }

    fn value_of(&mut self, name: &str) -> Option<&'static str> {
        self.view()
            .devices
            .into_iter()
            .find(|d| d.name == name)
            .unwrap_or_else(|| panic!("{name} is not in view"))
            .value
    }

    fn lit_name(&mut self) -> Option<String> {
        let view = self.view();
        match view.lit {
            Lit::Device(i) => Some(view.devices[i].name.clone()),
            Lit::Switch => None,
        }
    }
}

#[test]
fn bluetooth_is_listed_under_network_and_opens_its_screen() {
    let mut r = Rig::new();
    let general: Vec<SettingsRow> =
        r.a.settings_rows()
            .into_iter()
            .filter(|row| row.group() == omega_ui::SettingsGroup::General)
            .collect();
    assert_eq!(
        general,
        [
            SettingsRow::DateTime,
            SettingsRow::Sleep,
            SettingsRow::Network,
            SettingsRow::Bluetooth,
            SettingsRow::Background,
            SettingsRow::About,
        ]
    );
    assert_eq!(SettingsRow::Bluetooth.label(), "Bluetooth");
    assert!(SettingsRow::Bluetooth.opens() && !SettingsRow::Bluetooth.carets());
    r.open();
    assert_eq!(r.sel(), Sel::Switch);
}

#[test]
fn settings_slides_deeper_into_bluetooth_and_b_slides_back_to_its_row() {
    let mut r = Rig::new();
    r.open_menu_at(SettingsRow::Bluetooth);
    r.a.finish_slide();
    r.press(Btn::A);
    let slide = r.a.slide().expect("Bluetooth did not slide in");
    assert_eq!(slide.dir, Dir::Deeper);
    assert!(matches!(
        slide.from,
        Phase::Settings {
            row: SettingsRow::Bluetooth
        }
    ));
    r.a.finish_slide();
    r.press(Btn::B);
    assert_eq!(r.a.settings_row(), Some(SettingsRow::Bluetooth));
    let slide = r.a.slide().expect("Settings did not slide back");
    assert_eq!(slide.dir, Dir::Back);
    assert!(matches!(slide.from, Phase::Bluetooth(_)));
    r.a.finish_slide();
    r.press(Btn::A);
    r.a.apply(Action::QuickMenu);
    assert_eq!(
        r.a.settings_row(),
        Some(SettingsRow::Bluetooth),
        "MENU did not come back"
    );
}

#[test]
fn a_handheld_without_bluetooth_has_no_bluetooth_row_or_icon() {
    let mut r = Rig::with(AppState::default(), Radio::absent());
    assert!(!r.a.settings_rows().contains(&SettingsRow::Bluetooth));
    assert!(r
        .a
        .settings_shown()
        .iter()
        .all(|s| s.row != SettingsRow::Bluetooth));
    r.open_menu_at(SettingsRow::Network);
    r.press(Btn::Down);
    assert_eq!(r.a.settings_row(), Some(SettingsRow::Background));
    r.press(Btn::Up);
    assert_eq!(r.a.settings_row(), Some(SettingsRow::Network));
    assert_eq!(r.a.links().bluetooth, None, "an icon for a radio it lacks");

    let r = Rig::with(
        AppState {
            bluetooth: true,
            ..AppState::default()
        },
        Radio::absent(),
    );
    assert_eq!(r.radio.asked(), [], "brought up a radio it lacks");
}

#[test]
fn the_row_says_off_on_or_the_device_connected_in_capitals() {
    let mut r = Rig::new();
    assert_eq!(r.a.bluetooth_summary(), Summary::Off);
    assert_eq!(
        r.a.settings_text(SettingsRow::Bluetooth),
        Some("OFF".into())
    );
    r.on_with(&[]);
    assert_eq!(r.a.settings_text(SettingsRow::Bluetooth), Some("ON".into()));
    r.radio.say(status(&flows()));
    r.tick(16);
    assert_eq!(
        r.a.bluetooth_summary(),
        Summary::Connected("8BitDo SN30 Pro".into())
    );
    assert_eq!(
        r.a.settings_text(SettingsRow::Bluetooth),
        Some("8BITDO SN30 PRO".into())
    );
    let mut paired_only = flows();
    paired_only[0].connected = false;
    r.radio.say(status(&paired_only));
    r.tick(16);
    assert_eq!(r.a.settings_text(SettingsRow::Bluetooth), Some("ON".into()));
}

#[test]
fn switching_bluetooth_brings_the_radio_up_or_down_and_is_remembered() {
    let mut r = Rig::new();
    r.open();
    r.press(Btn::A);
    assert_eq!(r.radio.asked(), [BluetoothJob::Up]);
    assert!(read_app_state(r.d.path()).bluetooth, "on was not saved");
    r.press(Btn::Left);
    assert_eq!(r.radio.asked(), [BluetoothJob::Up, BluetoothJob::Down]);
    assert!(!read_app_state(r.d.path()).bluetooth, "off was not saved");
    r.press(Btn::Right);
    assert_eq!(r.radio.asked().last(), Some(&BluetoothJob::Up));
    assert_eq!(
        r.view().switch,
        SettingLine {
            label: "BLUETOOTH",
            value: Some("ON"),
            carets: true,
            opens: false,
        }
    );
}

#[test]
fn a_handheld_that_left_bluetooth_on_brings_it_up_at_boot_without_waiting() {
    let r = Rig::on();
    assert_eq!(r.radio.asked(), [BluetoothJob::Up]);
    assert!(r.a.bluetooth().enabled());
    let off = Rig::new();
    assert_eq!(off.radio.asked(), [], "Bluetooth off came up at boot");

    // the boot itself, with the stand-in radio: on from the first frame, the radio coming up
    // behind it
    let d = tmp_root_with_carts(&["Emerald"]);
    write_app_state(
        d.path(),
        &AppState {
            clock_set: true,
            bluetooth: true,
            ..AppState::default()
        },
    )
    .unwrap();
    let (a, _) = app_booting_at(d.path(), CLOCK_IS_SET);
    assert!(a.bluetooth().enabled(), "boot left Bluetooth off");
    assert_eq!(a.links().bluetooth, Some(true));
}

#[test]
fn the_top_bar_lights_bluetooth_while_it_is_on() {
    let mut r = Rig::new();
    assert_eq!(r.a.links().bluetooth, Some(false), "not shown off");
    r.open();
    r.press(Btn::A);
    assert_eq!(r.a.links().bluetooth, Some(true), "not lit once on");
    r.press(Btn::A);
    assert_eq!(r.a.links().bluetooth, Some(false), "still lit once off");
}

#[test]
fn opening_the_screen_never_looks_and_lists_only_the_paired_devices() {
    let mut r = Rig::on();
    r.radio.forget_asked();
    r.tick(5_000);
    assert_eq!(r.radio.asked(), [], "the shelf asked the radio something");
    r.open_menu_at(SettingsRow::Bluetooth);
    r.tick(16);
    assert_eq!(
        r.radio.asked(),
        [BluetoothJob::Refresh],
        "Settings did not ask which device is connected"
    );
    r.radio.say(status(&flows()));
    r.tick(16);
    r.radio.forget_asked();
    r.press(Btn::A);
    r.tick(16);
    assert_eq!(r.scans(), [], "opening the screen looked for devices");
    // the radio still knows the devices an earlier look found: only the paired ones are listed
    r.radio.say(status(&flows()));
    r.tick(16);
    assert_eq!(r.names(), ["8BitDo SN30 Pro", "WH-1000XM4"]);
    assert_eq!(r.notes(), ["Press X to look for devices."]);
    r.radio.forget_asked();
    r.tick(REFRESH_MS);
    assert_eq!(
        r.radio.asked(),
        [BluetoothJob::Refresh],
        "the list went stale"
    );
    r.answering(&flows(), 120_000);
    assert_eq!(r.scans(), [], "the screen looked for devices by itself");
    assert_eq!(r.names(), ["8BitDo SN30 Pro", "WH-1000XM4"]);
}

#[test]
fn with_no_device_paired_the_note_on_x_stands_alone() {
    let mut r = Rig::new();
    r.opened_with(&[]);
    let view = r.view();
    assert!(view.devices.is_empty(), "{:?}", view.devices);
    assert_eq!(view.notes, ["Press X to look for devices."]);
}

#[test]
fn x_looks_once_and_the_look_stops_by_itself() {
    let mut r = Rig::new();
    r.opened_with(&paired());
    r.press(Btn::X);
    assert_eq!(r.radio.asked(), [BluetoothJob::Scan(true)]);
    assert!(r.a.bluetooth().looking());
    assert_eq!(r.notes(), ["Looking for devices…"]);
    // what it finds joins the list after the paired devices, the note staying while it looks
    r.radio.say(status(&flows()[..3]));
    r.tick(16);
    assert_eq!(r.names(), ["8BitDo SN30 Pro", "WH-1000XM4", "Keychron K3"]);
    assert_eq!(r.notes(), ["Looking for devices…"]);
    r.answering(&flows(), LOOK_MS - 100);
    assert_eq!(r.scans(), [true], "the look ended early");
    assert_eq!(
        r.names(),
        [
            "8BitDo SN30 Pro",
            "WH-1000XM4",
            "Keychron K3",
            "Mi Smart Band 4"
        ]
    );
    r.answering(&flows(), 100);
    assert_eq!(r.scans(), [true, false], "the look did not stop by itself");
    assert!(!r.a.bluetooth().looking());
    assert!(r.notes().is_empty(), "{:?}", r.notes());
    // and nothing looks again by itself, however long the screen stays open
    r.answering(&flows(), 120_000);
    assert_eq!(r.scans(), [true, false], "a look came round by itself");
    assert_eq!(r.names().len(), 4);
}

#[test]
fn x_is_ignored_while_looking_or_with_bluetooth_off_and_x_again_looks_again() {
    let mut r = Rig::new();
    r.open();
    r.press(Btn::X);
    r.tick(16);
    assert_eq!(r.radio.asked(), [], "X looked with Bluetooth off");
    assert!(!r.a.bluetooth().looking());

    r.press(Btn::A);
    r.radio.say(status(&paired()));
    r.tick(16);
    r.radio.forget_asked();
    r.press(Btn::X);
    r.tick(1_000);
    r.press(Btn::X);
    r.tick(1_000);
    r.press(Btn::X);
    assert_eq!(r.scans(), [true], "X while looking looked again");
    r.tick(LOOK_MS);
    assert_eq!(r.scans(), [true, false]);
    r.press(Btn::X);
    assert_eq!(r.scans(), [true, false, true], "X did not look again");
    assert_eq!(r.notes(), ["Looking for devices…"]);
}

#[test]
fn x_waits_while_a_device_pairs_or_connects() {
    let mut r = Rig::new();
    r.on_with(&flows());
    r.go_to("WH-1000XM4");
    r.press(Btn::A);
    r.press(Btn::X);
    assert_eq!(r.scans(), [], "X looked while a device connected");
    let mut now = flows();
    now[1].connected = true;
    r.radio.say(done(connect(CANS), Ok(())));
    r.radio.say(status(&now));
    r.tick(16);
    r.press(Btn::X);
    assert_eq!(r.scans(), [true]);
}

#[test]
fn leaving_stops_a_look_and_asks_nothing_more() {
    let mut r = Rig::new();
    r.opened_with(&paired());
    r.press(Btn::X);
    r.radio.say(status(&flows()));
    r.tick(16);
    r.radio.forget_asked();
    r.press(Btn::B);
    r.tick(16);
    assert_eq!(r.scans(), [false], "leaving did not stop the look");
    r.answering(&flows(), LOOK_MS * 2);
    assert_eq!(
        r.scans(),
        [false],
        "the radio was asked about discovery again"
    );
    // what the look found goes with it: the screen opens again on the paired devices alone
    r.press(Btn::A);
    r.radio.say(status(&flows()));
    r.tick(16);
    assert_eq!(r.names(), ["8BitDo SN30 Pro", "WH-1000XM4"]);
    assert_eq!(r.notes(), ["Press X to look for devices."]);
    assert_eq!(r.scans(), [false]);

    // leaving once the look is over asks nothing about discovery
    let mut r = Rig::new();
    r.on_with(&flows());
    r.press(Btn::B);
    r.answering(&flows(), 1_000);
    assert_eq!(r.scans(), [], "leaving stopped a look that was over");
}

#[test]
fn switching_off_mid_look_ends_it_and_what_it_found() {
    let mut r = Rig::new();
    r.opened_with(&paired());
    r.press(Btn::X);
    r.radio.say(status(&flows()));
    r.tick(16);
    r.press(Btn::A);
    assert_eq!(r.radio.asked().last(), Some(&BluetoothJob::Down));
    assert!(!r.a.bluetooth().looking());
    r.radio.forget_asked();
    r.tick(LOOK_MS);
    assert_eq!(
        r.scans(),
        [],
        "the look was stopped after Bluetooth went down"
    );
    r.press(Btn::A);
    r.radio.say(status(&flows()));
    r.tick(16);
    assert_eq!(r.names(), ["8BitDo SN30 Pro", "WH-1000XM4"]);
    assert_eq!(r.notes(), ["Press X to look for devices."]);
}

#[test]
fn the_bar_offers_x_scan_while_bluetooth_is_on_and_y_forget_beside_it() {
    let mut r = Rig::new();
    r.open();
    assert_eq!(r.a.bar_hints(r.a.phase()), [], "X SCAN with Bluetooth off");
    r.press(Btn::A);
    r.radio.say(status(&paired()));
    r.tick(16);
    assert_eq!(r.a.bar_hints(r.a.phase()), [SCAN]);
    assert_eq!((SCAN.key, SCAN.word), ("X", "SCAN"));
    r.press(Btn::X);
    r.radio.say(status(&flows()));
    r.tick(16);
    assert_eq!(
        r.a.bar_hints(r.a.phase()),
        [SCAN],
        "no X SCAN while looking"
    );
    r.go_to("8BitDo SN30 Pro");
    assert_eq!(r.a.bar_hints(r.a.phase()), [SCAN, FORGET]);
    r.go_to("WH-1000XM4");
    assert_eq!(r.a.bar_hints(r.a.phase()), [SCAN, FORGET]);
    r.go_to("Keychron K3");
    assert_eq!(
        r.a.bar_hints(r.a.phase()),
        [SCAN],
        "Y FORGET on a device found"
    );
}

#[test]
fn a_look_that_finds_nothing_says_so() {
    let mut r = Rig::new();
    r.opened_with(&paired());
    r.press(Btn::X);
    r.answering(&paired(), LOOK_MS);
    assert!(!r.a.bluetooth().looking());
    assert_eq!(r.notes(), ["No devices found."]);
    assert_eq!(r.names(), ["8BitDo SN30 Pro", "WH-1000XM4"]);
}

#[test]
fn a_device_found_is_listed_by_its_name_once_it_has_one_and_never_without() {
    let mut r = Rig::new();
    r.opened_with(&paired());
    r.press(Btn::X);
    let mut nameless = paired();
    nameless.push(device(KEYS, "", DeviceKind::Keyboard, false, false));
    nameless.push(device(ODD, "", DeviceKind::Other, false, false));
    r.radio.say(status(&nameless));
    r.tick(16);
    assert_eq!(r.names(), ["8BitDo SN30 Pro", "WH-1000XM4"]);
    // its name comes a few seconds after it was first seen, the beacon's never
    let mut named = nameless.clone();
    named[2].name = "Keychron K3".into();
    r.answering(&named, LOOK_MS);
    assert_eq!(r.names(), ["8BitDo SN30 Pro", "WH-1000XM4", "Keychron K3"]);
    // a name that comes only once the look is over is taken too: the status after it is asked
    let mut late = named.clone();
    late.push(device(
        BAND,
        "Mi Smart Band 4",
        DeviceKind::Other,
        false,
        false,
    ));
    r.answering(&late, REFRESH_MS * 2);
    assert_eq!(
        r.names(),
        [
            "8BitDo SN30 Pro",
            "WH-1000XM4",
            "Keychron K3",
            "Mi Smart Band 4"
        ]
    );
    assert!(
        !r.names().contains(&ODD.to_string()),
        "the beacon is listed"
    );
}

#[test]
fn a_paired_device_with_no_name_is_listed_by_its_address() {
    let mut r = Rig::new();
    r.opened_with(&[device(ODD, "", DeviceKind::Other, true, false)]);
    assert_eq!(r.names(), [ODD]);
}

#[test]
fn what_a_look_found_stays_listed_once_bluetoothd_forgets_it() {
    let mut r = Rig::new();
    r.on_with(&flows());
    // bluetoothd forgets the devices only found soon after the look stops
    r.answering(&paired(), 60_000);
    assert_eq!(
        r.names(),
        [
            "8BitDo SN30 Pro",
            "WH-1000XM4",
            "Keychron K3",
            "Mi Smart Band 4"
        ]
    );
    r.go_to("Keychron K3");
    r.radio.forget_asked();
    r.press(Btn::A);
    assert_eq!(r.radio.asked(), [BluetoothJob::Pair(KEYS.into())]);
    assert_eq!(r.value_of("Keychron K3"), Some("PAIRING"));
    // a look again lets go of what bluetoothd no longer knows, and finds what is still in reach
    r.radio.say(done(pair(KEYS), Err(Fail::Pair)));
    r.tick(16);
    r.press(Btn::X);
    assert_eq!(r.names(), ["8BitDo SN30 Pro", "WH-1000XM4"]);
    let mut band = paired();
    band.push(flows()[3].clone());
    r.radio.say(status(&band));
    r.tick(16);
    assert_eq!(
        r.names(),
        ["8BitDo SN30 Pro", "WH-1000XM4", "Mi Smart Band 4"]
    );
}

#[test]
fn with_bluetooth_off_the_screen_looks_for_nothing_and_says_so() {
    let mut r = Rig::new();
    r.open();
    r.tick(16);
    r.tick(LOOK_MS * 5);
    assert_eq!(r.radio.asked(), [], "asked the radio with Bluetooth off");
    let view = r.view();
    assert_eq!(view.switch.value, Some("OFF"));
    assert!(view.devices.is_empty(), "devices with Bluetooth off");
    assert_eq!(view.notes, ["Bluetooth is off."]);
    assert_eq!(view.lit, Lit::Switch);
}

#[test]
fn a_radio_that_never_comes_up_says_so_until_switched_again() {
    let mut r = Rig::new();
    r.open();
    r.press(Btn::A);
    r.radio.say(BluetoothNews::Started(false));
    r.tick(16);
    assert_eq!(r.notes(), ["The Bluetooth radio did not start."]);
    r.tick(NOTE_MS * 4);
    assert_eq!(r.notes(), ["The Bluetooth radio did not start."]);
    r.press(Btn::A);
    r.press(Btn::A);
    assert_eq!(r.notes(), ["Press X to look for devices."]);
}

#[test]
fn the_list_holds_the_paired_devices_then_the_found_ones_with_a_name() {
    let mut r = Rig::new();
    let mut devices = flows();
    devices.reverse();
    devices.push(device(
        "AA:BB:CC:00:00:01",
        "anker soundcore",
        DeviceKind::Audio,
        false,
        false,
    ));
    devices.push(device(
        "AA:BB:CC:00:00:02",
        "Arcade Stick",
        DeviceKind::Gamepad,
        true,
        false,
    ));
    devices.push(device(
        "00:00:00:00:00:09",
        "",
        DeviceKind::Other,
        false,
        false,
    ));
    r.on_with(&devices);
    let line = |name: &str, value, kind| DeviceLine {
        name: name.into(),
        value,
        kind,
    };
    assert_eq!(
        r.view().devices,
        [
            line("8BitDo SN30 Pro", Some("CONNECTED"), DeviceKind::Gamepad),
            line("Arcade Stick", Some("PAIRED"), DeviceKind::Gamepad),
            line("WH-1000XM4", Some("PAIRED"), DeviceKind::Audio),
            line("anker soundcore", None, DeviceKind::Audio),
            line("Keychron K3", None, DeviceKind::Keyboard),
            line("Mi Smart Band 4", None, DeviceKind::Other),
        ],
        "the names keep their case, and a device found with none is not listed"
    );
}

#[test]
fn a_on_a_found_device_pairs_trusts_and_connects_it() {
    let mut r = Rig::new();
    r.on_with(&flows());
    r.go_to("Keychron K3");
    assert!(
        !r.a.bluetooth_forgets(),
        "Y FORGET on a device never paired"
    );
    assert_eq!(r.a.bar_hints(r.a.phase()), [SCAN]);
    r.press(Btn::A);
    assert_eq!(r.radio.asked(), [BluetoothJob::Pair(KEYS.into())]);
    assert_eq!(r.value_of("Keychron K3"), Some("PAIRING"));
    assert!(r.notes().is_empty(), "{:?}", r.notes());
    // A again while it pairs, or on another device, asks for nothing more
    r.press(Btn::A);
    r.press(Btn::Up);
    r.press(Btn::A);
    assert_eq!(r.radio.asked(), [BluetoothJob::Pair(KEYS.into())]);
    r.press(Btn::Down);

    let mut now = flows();
    now[2].paired = true;
    now[2].trusted = true;
    now[2].connected = true;
    r.radio.say(done(pair(KEYS), Ok(())));
    r.radio.say(status(&now));
    r.tick(16);
    assert_eq!(r.value_of("Keychron K3"), Some("CONNECTED"));
    assert_eq!(
        r.lit_name().as_deref(),
        Some("Keychron K3"),
        "the cursor did not follow the keyboard up the list"
    );
    assert_eq!(r.scans(), [], "the pairing was followed by a look");
}

#[test]
fn a_pairing_ends_the_look_under_way() {
    let mut r = Rig::new();
    r.opened_with(&paired());
    r.press(Btn::X);
    r.radio.say(status(&flows()));
    r.tick(16);
    r.go_to("Keychron K3");
    r.radio.forget_asked();
    r.press(Btn::A);
    // the script stops discovery before it pairs
    assert_eq!(r.radio.asked(), [BluetoothJob::Pair(KEYS.into())]);
    assert!(!r.a.bluetooth().looking());
    assert!(r.notes().is_empty(), "{:?}", r.notes());
    r.tick(LOOK_MS);
    assert_eq!(
        r.scans(),
        [],
        "discovery was turned off again after the pairing"
    );
    assert_eq!(r.value_of("Keychron K3"), Some("PAIRING"));
}

#[test]
fn a_on_a_paired_device_connects_it_and_on_a_connected_one_disconnects_it() {
    let mut r = Rig::new();
    r.on_with(&flows());
    r.go_to("WH-1000XM4");
    r.press(Btn::A);
    assert_eq!(r.radio.asked(), [BluetoothJob::Connect(CANS.into())]);
    assert_eq!(r.value_of("WH-1000XM4"), Some("CONNECTING"));
    assert!(
        r.a.bluetooth_forgets(),
        "no Y FORGET on a paired device being connected"
    );
    let mut now = flows();
    now[1].connected = true;
    r.radio.say(done(connect(CANS), Ok(())));
    r.radio.say(status(&now));
    r.tick(16);
    assert_eq!(r.value_of("WH-1000XM4"), Some("CONNECTED"));
    r.radio.forget_asked();

    r.go_to("8BitDo SN30 Pro");
    r.press(Btn::A);
    assert_eq!(r.radio.asked(), [BluetoothJob::Disconnect(PAD.into())]);
}

#[test]
fn left_and_right_on_a_device_do_nothing() {
    let mut r = Rig::new();
    r.on_with(&flows());
    r.go_to("Keychron K3");
    r.press(Btn::Left);
    r.press(Btn::Right);
    assert_eq!(r.radio.asked(), []);
    assert!(r.a.bluetooth().enabled());
}

#[test]
fn y_forgets_a_paired_device_and_nothing_else() {
    let mut r = Rig::new();
    r.on_with(&flows());
    r.press(Btn::Y);
    assert_eq!(r.radio.asked(), [], "Y on the switch forgot something");
    for found in ["Keychron K3", "Mi Smart Band 4"] {
        r.go_to(found);
        r.press(Btn::Y);
        assert_eq!(r.radio.asked(), [], "Y forgot {found}, never paired");
    }
    for (name, mac) in [("WH-1000XM4", CANS), ("8BitDo SN30 Pro", PAD)] {
        r.go_to(name);
        assert!(r.a.bluetooth_forgets(), "no Y FORGET on {name}");
        assert_eq!(r.a.bar_hints(r.a.phase()), [SCAN, FORGET]);
        r.press(Btn::Y);
        assert_eq!(r.radio.asked(), [BluetoothJob::Forget(mac.into())]);
        assert!(
            !r.names().contains(&name.to_string()),
            "{name} is still listed"
        );
        // a status from before the forget does not bring it back
        r.radio.say(status(&flows()));
        r.tick(16);
        assert!(!r.names().contains(&name.to_string()), "{name} came back");
        r.radio.say(done(BluetoothJob::Forget(mac.into()), Ok(())));
        r.radio.say(status(
            &flows()
                .into_iter()
                .filter(|d| d.mac != CANS && d.mac != mac)
                .collect::<Vec<_>>(),
        ));
        r.tick(16);
        assert!(!r.names().contains(&name.to_string()));
        r.radio.forget_asked();
    }
    assert_eq!(r.names(), ["Keychron K3", "Mi Smart Band 4"]);
}

#[test]
fn y_on_a_device_connecting_forgets_it_once_the_connection_is_done() {
    let mut r = Rig::new();
    r.on_with(&flows());
    r.go_to("WH-1000XM4");
    r.press(Btn::A);
    r.press(Btn::Y);
    assert_eq!(
        r.radio.asked(),
        [
            BluetoothJob::Connect(CANS.into()),
            BluetoothJob::Forget(CANS.into())
        ]
    );
    assert!(!r.names().contains(&"WH-1000XM4".to_string()));
    // the connection answers first, and settles nothing: the forget is what is under way
    r.radio.say(done(connect(CANS), Err(Fail::Connect)));
    r.radio.say(status(&flows()));
    r.tick(16);
    assert!(r.notes().is_empty(), "{:?}", r.notes());
    assert!(!r.names().contains(&"WH-1000XM4".to_string()));
    r.radio.say(done(BluetoothJob::Forget(CANS.into()), Ok(())));
    r.tick(16);
    assert!(r.a.bluetooth().doing().is_none());
}

#[test]
fn a_pairing_that_fails_says_so_for_a_moment() {
    let mut r = Rig::new();
    r.on_with(&flows());
    r.go_to("Keychron K3");
    r.press(Btn::A);
    r.radio.say(done(pair(KEYS), Err(Fail::Pair)));
    r.tick(16);
    assert_eq!(r.notes(), ["Could not pair with Keychron K3."]);
    assert_eq!(r.value_of("Keychron K3"), None, "still pairing");
    r.tick(NOTE_MS);
    assert!(r.notes().is_empty(), "the note stayed: {:?}", r.notes());

    // paired, but the connection after it failed
    r.press(Btn::A);
    r.radio.say(done(pair(KEYS), Err(Fail::Connect)));
    r.tick(16);
    assert_eq!(r.notes(), ["Could not connect to Keychron K3."]);
}

#[test]
fn a_connection_that_fails_says_so_for_a_moment() {
    let mut r = Rig::new();
    r.on_with(&flows());
    r.go_to("WH-1000XM4");
    r.press(Btn::A);
    r.radio.say(done(connect(CANS), Err(Fail::Connect)));
    r.tick(16);
    assert_eq!(r.notes(), ["Could not connect to WH-1000XM4."]);
    assert_eq!(r.value_of("WH-1000XM4"), Some("PAIRED"));
    // a new attempt takes the note away
    r.press(Btn::A);
    assert!(r.notes().is_empty());
}

#[test]
fn a_paired_device_without_a_name_is_named_by_its_address_in_a_note() {
    let mut r = Rig::new();
    let mut devices = flows();
    devices.push(device(ODD, "", DeviceKind::Other, true, false));
    r.on_with(&devices);
    r.go_to(ODD);
    r.press(Btn::A);
    assert_eq!(r.radio.asked(), [BluetoothJob::Connect(ODD.into())]);
    r.radio.say(done(connect(ODD), Err(Fail::Connect)));
    r.tick(16);
    assert_eq!(r.notes(), [format!("Could not connect to {ODD}.")]);
}

#[test]
fn a_long_name_is_cut_short_in_a_note() {
    let mut r = Rig::new();
    let long = "The wireless headphones with the very long name";
    r.on_with(&[device(CANS, long, DeviceKind::Audio, true, false)]);
    r.go_to(long);
    r.press(Btn::A);
    r.radio.say(done(connect(CANS), Err(Fail::Connect)));
    r.tick(16);
    let notes = r.notes();
    assert_eq!(notes.len(), 1);
    assert!(
        notes[0].starts_with("Could not connect to The wireless") && notes[0].ends_with("…."),
        "{:?}",
        notes[0]
    );
    assert!(
        notes[0].chars().count() <= 43,
        "{:?} is wider than the list's panel at 640x480",
        notes[0]
    );
    assert_eq!(r.names(), [long], "the list cut the name; its row cuts it");
}

#[test]
fn a_job_that_never_answers_gives_up() {
    let mut r = Rig::new();
    r.on_with(&flows());
    r.go_to("Keychron K3");
    r.press(Btn::A);
    r.tick(DOING_MS / 2);
    assert_eq!(r.value_of("Keychron K3"), Some("PAIRING"));
    r.tick(DOING_MS);
    assert_eq!(r.notes(), ["Could not pair with Keychron K3."]);
    assert_eq!(r.value_of("Keychron K3"), None);
}

#[test]
fn switching_off_drops_the_list_and_whatever_was_under_way() {
    let mut r = Rig::new();
    r.on_with(&flows());
    r.go_to("Keychron K3");
    r.press(Btn::A);
    for _ in 0..10 {
        r.press(Btn::Up);
        if r.sel() == Sel::Switch {
            break;
        }
    }
    r.press(Btn::A);
    assert_eq!(r.radio.asked().last(), Some(&BluetoothJob::Down));
    assert!(r.a.bluetooth().doing().is_none());
    assert!(r.names().is_empty());
    assert_eq!(r.notes(), ["Bluetooth is off."]);
    // a status the radio sent before it went down changes nothing
    r.radio.say(status(&flows()));
    r.tick(16);
    assert!(r.names().is_empty());
}

#[test]
fn the_cursor_stays_on_its_device_when_the_list_reorders() {
    let mut r = Rig::new();
    r.on_with(&flows());
    r.go_to("WH-1000XM4");
    let mut now = flows();
    now.push(device(
        "AA:BB:CC:00:00:01",
        "Arcade Stick",
        DeviceKind::Gamepad,
        true,
        false,
    ));
    now.push(device(
        "AA:BB:CC:00:00:02",
        "Joy-Con (L)",
        DeviceKind::Gamepad,
        false,
        false,
    ));
    r.radio.say(status(&now));
    r.tick(16);
    assert_eq!(r.lit_name().as_deref(), Some("WH-1000XM4"));
    // the device under the cursor gone, forgotten by another, it settles on the row where it was
    let gone: Vec<Device> = now.iter().filter(|d| d.mac != CANS).cloned().collect();
    r.radio.say(status(&gone));
    r.tick(16);
    assert_eq!(r.lit_name().as_deref(), Some("Joy-Con (L)"));
}

#[test]
fn a_list_longer_than_its_panel_scrolls_a_whole_row_at_a_time() {
    let mut r = Rig::new();
    let many: Vec<Device> = (0..30)
        .map(|i| {
            device(
                &format!("AA:BB:CC:00:00:{i:02}"),
                &format!("Pad {i:02}"),
                DeviceKind::Gamepad,
                false,
                false,
            )
        })
        .collect();
    r.on_with(&many);
    let shown = networks_shown(1, 0);
    assert!(shown < 30, "the panel holds all {shown}");
    assert_eq!(r.names().len(), shown, "the panel is not full");
    assert_eq!(r.names()[0], "Pad 00");
    r.go_to(&format!("Pad {:02}", shown - 1));
    assert_eq!(r.names()[0], "Pad 00");
    assert_eq!(r.view().lit, Lit::Device(shown - 1));
    r.press(Btn::Down);
    assert_eq!(r.names()[0], "Pad 01");
    assert_eq!(r.view().lit, Lit::Device(shown - 1));
    for _ in 0..shown - 1 {
        r.press(Btn::Up);
    }
    assert_eq!(r.names()[0], "Pad 01");
    r.press(Btn::Up);
    assert_eq!(r.names()[0], "Pad 00");
    assert_eq!(r.view().lit, Lit::Device(0));
    // up from the switch goes round to the last device, the list at its end
    r.press(Btn::Up);
    assert_eq!(r.view().lit, Lit::Switch);
    r.press(Btn::Up);
    assert_eq!(r.lit_name().as_deref(), Some("Pad 29"));
    assert_eq!(r.names()[0], format!("Pad {:02}", 30 - shown));
    // and down from there round to the switch, the list staying where it was
    r.press(Btn::Down);
    assert_eq!(r.view().lit, Lit::Switch);
    assert_eq!(r.names()[0], format!("Pad {:02}", 30 - shown));
}

#[test]
fn only_the_bluetooth_screen_and_settings_watch_the_radio() {
    let mut r = Rig::on();
    assert_eq!(r.a.bluetooth_watch(), Watch::Idle, "the shelf");
    r.open_menu_at(SettingsRow::Bluetooth);
    assert_eq!(r.a.bluetooth_watch(), Watch::Status);
    r.press(Btn::A);
    assert_eq!(r.a.bluetooth_watch(), Watch::Devices);
}
