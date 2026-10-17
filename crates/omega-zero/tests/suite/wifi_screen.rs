use crate::common;

use std::sync::{Arc, Mutex};

use common::{app_booting_at, tmp_root_with_carts, CLOCK_IS_SET};
use omega_input::{Action, Btn, Millis};
use omega_store::{read_app_state, write_app_state, AppState};
use omega_ui::kit::bar::Hint;
use omega_ui::kit::palette::palette;
use omega_ui::kit::row::row_width;
use omega_ui::kit::text::text_face;
use omega_ui::network::networks_shown;
use omega_ui::network::{info_chrome, network_chrome, NOTE_IN};
use omega_ui::Layer;
use omega_ui::SettingsRow;
use omega_zero::app::{App, Phase};
use omega_zero::screens::bar::FORGET;
use omega_zero::screens::bar::INFO;
use omega_zero::screens::bar::SCAN;
use omega_zero::screens::info::{wifi_view, InfoView, NETWORK_SHOWN};
use omega_zero::wifi::{
    Saved, Seen, Status, WifiJob, WifiJobs, WifiNews, GLANCE_MS, JOIN_TIMEOUT_MS,
};
use omega_zero::wifi_screen::{Lit, NetworkLine, NetworkView, Sel, SettingLine, Summary, Wifi};
use tempfile::TempDir;

#[derive(Default)]
struct RadioState {
    absent: bool,
    asked: Vec<WifiJob>,
    news: Vec<WifiNews>,
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

    fn asked(&self) -> Vec<WifiJob> {
        self.0.lock().unwrap().asked.clone()
    }

    fn forget_asked(&self) {
        self.0.lock().unwrap().asked.clear();
    }

    fn say(&self, news: WifiNews) {
        self.0.lock().unwrap().news.push(news);
    }
}

impl WifiJobs for Radio {
    fn present(&self) -> bool {
        !self.0.lock().unwrap().absent
    }

    fn ask(&mut self, job: WifiJob) {
        self.0.lock().unwrap().asked.push(job);
    }

    fn news(&mut self) -> Vec<WifiNews> {
        std::mem::take(&mut self.0.lock().unwrap().news)
    }
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
        a.set_wifi_jobs(Box::new(radio.clone()));
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

    fn press(&mut self, btn: Btn) {
        self.a.apply(Action::GbaDown(btn));
        self.a.apply(Action::GbaUp(btn));
    }

    fn tick(&mut self, ms: Millis) {
        self.now += ms;
        self.a.tick_ms(self.now);
    }

    fn open_menu_at(&mut self, row: SettingsRow) {
        self.press(Btn::Start);
        assert!(self.a.settings_row().is_some(), "START opened no Settings");
        while self.a.settings_row() != Some(row) {
            self.press(Btn::Down);
            assert_ne!(
                self.a.settings_row(),
                Some(SettingsRow::FastForward),
                "the bar went round without reaching {row:?}"
            );
        }
    }

    fn open_wifi(&mut self) {
        self.open_menu_at(SettingsRow::Network);
        self.press(Btn::A);
        assert!(
            matches!(self.a.phase(), Phase::Network(_)),
            "A did not open Wi-Fi"
        );
    }

    /// Opens the Network screen with Wi-Fi on and these networks saved, and X's scan hearing
    /// these.
    fn on_with(&mut self, seen: &[(&str, i32, bool)], saved: &[(u32, &str)]) {
        self.on_saved(saved);
        self.radio.forget_asked();
        self.press(Btn::X);
        assert_eq!(self.radio.asked(), [WifiJob::Scan], "X did not scan");
        self.radio.say(heard(seen));
        self.tick(16);
        self.radio.forget_asked();
    }

    /// Opens the Network screen with Wi-Fi on and these networks saved, before any scan.
    fn on_saved(&mut self, saved: &[(u32, &str)]) {
        self.open_wifi();
        if !self.a.wifi().enabled() {
            self.press(Btn::A);
        }
        self.radio.say(WifiNews::Saved(
            saved
                .iter()
                .map(|(id, s)| Saved {
                    id: *id,
                    ssid: s.as_bytes().to_vec(),
                    current: false,
                    refused: false,
                })
                .collect(),
        ));
        self.tick(16);
        self.radio.forget_asked();
    }

    fn sel(&self) -> Sel {
        match self.a.phase() {
            Phase::Network(s) => s.sel.clone(),
            other => panic!("not on the Network screen: {other:?}"),
        }
    }

    fn go_to(&mut self, ssid: &str) {
        let want = Sel::Network(ssid.as_bytes().to_vec());
        for _ in 0..20 {
            if self.sel() == want {
                return;
            }
            self.press(Btn::Down);
        }
        panic!("{ssid} is not in the list");
    }

    fn typing(&self) -> Option<String> {
        self.a.typing().map(|t| t.keys.text.clone())
    }

    fn view(&mut self) -> NetworkView {
        self.a.network_view().expect("the Network modal is showing")
    }

    fn status(&mut self) -> Vec<String> {
        self.view().notes
    }

    /// The labels of NETWORK's rows, top first.
    fn settings(&mut self) -> Vec<&'static str> {
        self.view().settings.iter().map(|s| s.label).collect()
    }

    /// The names of the networks in view, top first.
    fn names(&mut self) -> Vec<String> {
        self.view().networks.into_iter().map(|n| n.name).collect()
    }

    /// The network's name under the cursor, when it is on one.
    fn lit_name(&mut self) -> Option<String> {
        let view = self.view();
        match view.lit {
            Lit::Network(i) => Some(view.networks[i].name.clone()),
            Lit::Setting(_) => None,
        }
    }
}

/// What a scan heard: these networks, each with its signal and whether it has a lock.
fn heard(seen: &[(&str, i32, bool)]) -> WifiNews {
    WifiNews::Seen(
        seen.iter()
            .map(|(s, rssi, secured)| Seen {
                ssid: s.as_bytes().to_vec(),
                rssi: *rssi,
                secured: *secured,
            })
            .collect(),
    )
}

/// How many scans the radio was asked for.
fn scans(radio: &Radio) -> usize {
    radio
        .asked()
        .iter()
        .filter(|job| **job == WifiJob::Scan)
        .count()
}

/// `n` networks in reach, the first strongest, all locked.
fn many(n: usize) -> Vec<(String, i32, bool)> {
    (0..n)
        .map(|i| (format!("Net {i:02}"), -40 - i as i32, true))
        .collect()
}

fn joined(ssid: &str, ip: Option<&str>) -> WifiNews {
    WifiNews::Status(Status {
        enabled: true,
        state: "COMPLETED".into(),
        ssid: Some(ssid.as_bytes().to_vec()),
        ip: ip.map(str::to_string),
        rssi: Some(-50),
        mac: Some("e4:5f:01:2a:9c:33".into()),
        hostname: "rg35xxsp".into(),
        ntp: true,
        ntp_synced: true,
    })
}

#[test]
fn a_on_wifi_opens_its_screen_and_b_comes_back_to_the_row() {
    let mut r = Rig::new();
    r.open_wifi();
    assert_eq!(r.sel(), Sel::Wifi);
    r.press(Btn::B);
    assert_eq!(r.a.settings_row(), Some(SettingsRow::Network));
    r.press(Btn::A);
    r.a.apply(Action::QuickMenu);
    assert_eq!(
        r.a.settings_row(),
        Some(SettingsRow::Network),
        "MENU did not come back"
    );
}

#[test]
fn a_handheld_without_a_radio_has_no_network_row() {
    let mut r = Rig::with(AppState::default(), Radio::absent());
    assert!(!r.a.settings_rows().contains(&SettingsRow::Network));
    r.open_menu_at(SettingsRow::Sleep);
    r.press(Btn::Down);
    assert_eq!(r.a.settings_row(), Some(SettingsRow::Bluetooth));
    r.press(Btn::Up);
    assert_eq!(r.a.settings_row(), Some(SettingsRow::Sleep));
}

#[test]
fn switching_wifi_on_brings_the_radio_up_and_is_remembered() {
    let mut r = Rig::new();
    r.open_wifi();
    r.press(Btn::A);
    assert_eq!(r.radio.asked(), [WifiJob::Up]);
    assert!(read_app_state(r.d.path()).wifi, "Wi-Fi on was not saved");
    r.press(Btn::Left);
    assert_eq!(r.radio.asked(), [WifiJob::Up, WifiJob::Down]);
    assert!(!read_app_state(r.d.path()).wifi, "Wi-Fi off was not saved");
}

#[test]
fn a_handheld_that_left_wifi_on_brings_it_up_at_boot() {
    let r = Rig::with(
        AppState {
            wifi: true,
            ..AppState::default()
        },
        Radio::default(),
    );
    assert_eq!(r.radio.asked(), [WifiJob::Up]);
    let off = Rig::new();
    assert_eq!(off.radio.asked(), [], "Wi-Fi off came up at boot");
}

#[test]
fn the_screen_and_the_menu_only_ask_how_it_is_joined() {
    let mut r = Rig::with(
        AppState {
            wifi: true,
            ..AppState::default()
        },
        Radio::default(),
    );
    r.radio.forget_asked();
    r.tick(5_000);
    assert_eq!(
        r.radio.asked(),
        [WifiJob::Glance],
        "the shelf asked for more than a glance at the link"
    );
    r.radio.forget_asked();
    r.open_menu_at(SettingsRow::Network);
    r.tick(16);
    assert_eq!(r.radio.asked(), [WifiJob::Refresh]);
    r.tick(5_000);
    assert_eq!(
        r.radio.asked(),
        [WifiJob::Refresh],
        "asked again before the last one was answered"
    );
    r.radio.say(WifiNews::Saved(Vec::new()));
    r.tick(16);
    r.tick(3_000);
    assert_eq!(r.radio.asked(), [WifiJob::Refresh, WifiJob::Refresh]);
    r.radio.forget_asked();
    r.press(Btn::A);
    r.tick(16);
    assert_eq!(
        r.radio.asked(),
        [],
        "opening the screen asked for more than the status already asked"
    );
}

/// The Network screen opened and left open asks for the status as Settings does, and never for
/// a scan: a scan ties the radio up, and only X asks for one.
#[test]
fn opening_network_never_scans() {
    let mut r = Rig::with(
        AppState {
            wifi: true,
            ..AppState::default()
        },
        Radio::default(),
    );
    r.open_wifi();
    r.radio.forget_asked();
    let mut refreshes = 0;
    for _ in 0..60 {
        for job in r.radio.asked() {
            assert_ne!(job, WifiJob::Scan, "the open screen scanned");
            if job == WifiJob::Refresh {
                refreshes += 1;
                r.radio.say(joined("Home", Some("192.168.1.20")));
                r.radio.say(WifiNews::Saved(Vec::new()));
            }
        }
        r.radio.forget_asked();
        r.tick(16);
        r.tick(1_000);
    }
    assert!(refreshes > 10, "the status was not kept fresh: {refreshes}");
    assert_eq!(r.status(), ["Press X to look for networks."]);
}

#[test]
fn x_scans_once_and_only_x_scans_again() {
    let mut r = Rig::new();
    r.on_saved(&[]);
    r.radio.forget_asked();
    r.press(Btn::X);
    assert_eq!(r.radio.asked(), [WifiJob::Scan]);
    assert_eq!(r.status(), ["Looking for networks…"]);
    r.radio
        .say(heard(&[("Home", -48, true), ("Café", -60, false)]));
    r.tick(16);
    assert_eq!(r.names(), ["Home", "Café"]);
    assert_eq!(r.status(), ["Not connected. Pick a network to join."]);
    // a minute on, its status kept fresh, it has not looked again
    for _ in 0..60 {
        r.radio.say(WifiNews::Saved(Vec::new()));
        r.tick(1_000);
    }
    assert_eq!(scans(&r.radio), 1, "it scanned again by itself");
    assert_eq!(r.names(), ["Home", "Café"], "what it found went away");
    r.radio.forget_asked();
    r.press(Btn::X);
    assert_eq!(
        r.radio.asked(),
        [WifiJob::Scan],
        "X again did not look again"
    );
    r.radio.say(heard(&[("Café", -50, false)]));
    r.tick(16);
    assert_eq!(
        r.names(),
        ["Café"],
        "the second scan did not replace the first"
    );
}

#[test]
fn x_does_nothing_while_a_scan_runs_or_with_wifi_off() {
    let mut r = Rig::new();
    r.open_wifi();
    r.press(Btn::X);
    assert_eq!(r.radio.asked(), [], "X scanned with Wi-Fi off");
    assert_eq!(r.status(), ["Wi-Fi is off."]);
    r.press(Btn::A);
    r.radio.forget_asked();
    r.press(Btn::X);
    r.press(Btn::X);
    r.tick(16);
    r.press(Btn::X);
    assert_eq!(scans(&r.radio), 1, "X asked again mid-scan");
    r.radio.say(heard(&[("Home", -48, true)]));
    r.tick(16);
    r.press(Btn::X);
    assert_eq!(scans(&r.radio), 2, "X after the answer did not look again");
}

#[test]
fn x_scan_shows_on_the_bar_with_wifi_on_and_y_info_or_y_forget_beside_it() {
    assert_eq!(
        SCAN,
        Hint {
            key: "X",
            word: "SCAN",
        }
    );
    let mut r = Rig::new();
    r.open_wifi();
    r.a.finish_slide();
    assert_eq!(r.a.bar_hints_shown(), [INFO], "X SCAN with Wi-Fi off");
    r.press(Btn::A);
    assert_eq!(r.a.bar_hints_shown(), [SCAN, INFO]);
    r.press(Btn::X);
    assert_eq!(
        r.a.bar_hints_shown(),
        [SCAN, INFO],
        "the hint left mid-scan"
    );
    r.radio.say(heard(&[
        ("Home", -48, true),
        ("Café", -60, false),
        ("Attic", -80, true),
    ]));
    r.radio.say(WifiNews::Saved(vec![Saved {
        id: 2,
        ssid: b"Home".to_vec(),
        current: false,
        refused: false,
    }]));
    r.tick(16);
    for row in [Sel::Clock, Sel::Ssh] {
        r.press(Btn::Down);
        assert_eq!(r.sel(), row);
        assert_eq!(r.a.bar_hints_shown(), [SCAN, INFO], "on {row:?}");
    }
    r.go_to("Home");
    assert_eq!(
        r.a.bar_hints_shown(),
        [SCAN, FORGET],
        "not side by side on a saved network"
    );
    r.go_to("Café");
    assert_eq!(r.a.bar_hints_shown(), [SCAN]);
    // the keyboard keeps its own keys, X among them for SHIFT
    r.go_to("Attic");
    r.press(Btn::A);
    r.a.finish_slide();
    assert!(r.a.network_typing().is_some(), "no keyboard");
    assert_eq!(
        r.a.bar_hints_shown(),
        [],
        "the bar's hints over the keyboard"
    );
    r.radio.forget_asked();
    r.press(Btn::X);
    assert_eq!(r.radio.asked(), [], "the keyboard's X scanned");
    assert_eq!(
        r.a.network_typing().unwrap().keys.layer,
        Layer::Upper,
        "X did not shift"
    );
}

#[test]
fn before_a_scan_the_list_holds_the_network_joined_and_the_saved_ones() {
    let mut r = Rig::new();
    r.on_saved(&[(0, "Home"), (4, "Cabin")]);
    r.radio.say(joined("Home", Some("192.168.1.20")));
    r.tick(16);
    let line = |name: &str, value| NetworkLine {
        name: name.into(),
        value,
        signal: None,
        lock: false,
    };
    assert_eq!(
        r.view().networks,
        [
            line("Home", Some("CONNECTED")),
            line("Cabin", Some("SAVED"))
        ],
        "a network showed a signal no scan heard"
    );
    assert_eq!(r.status(), ["Press X to look for networks."]);
    assert_eq!(scans(&r.radio), 0);
}

#[test]
fn the_network_the_status_names_is_listed_though_nothing_saved_names_it() {
    let mut r = Rig::new();
    r.on_saved(&[]);
    r.radio.say(joined("Home", None));
    r.tick(16);
    assert_eq!(r.names(), ["Home"]);
    assert_eq!(r.view().networks[0].value, Some("CONNECTED"));
    assert_eq!(r.status(), ["Waiting for an address…"]);
    r.radio.say(joined("Home", Some("192.168.1.20")));
    r.tick(16);
    assert_eq!(r.status(), ["Press X to look for networks."]);
}

#[test]
fn leaving_the_screen_scans_no_more_and_coming_back_starts_without_what_it_found() {
    let mut r = Rig::new();
    let home = || {
        WifiNews::Saved(vec![Saved {
            id: 0,
            ssid: b"Home".to_vec(),
            current: false,
            refused: false,
        }])
    };
    r.on_with(&[("Home", -48, true), ("Café", -60, false)], &[(0, "Home")]);
    assert_eq!(r.names(), ["Home", "Café"]);
    // left with a scan under way: it finishes, and what it found is for nobody
    r.press(Btn::X);
    r.press(Btn::B);
    assert_eq!(r.a.settings_row(), Some(SettingsRow::Network));
    r.radio.say(heard(&[("Upstairs", -40, true)]));
    r.tick(16);
    r.radio.forget_asked();
    for _ in 0..60 {
        r.radio.say(home());
        r.tick(1_000);
    }
    assert_eq!(scans(&r.radio), 0, "Settings scanned");
    r.press(Btn::A);
    assert!(matches!(r.a.phase(), Phase::Network(_)));
    assert_eq!(r.names(), ["Home"], "came back to what an old scan found");
    assert_eq!(r.view().networks[0].signal, None);
    assert_eq!(r.status(), ["Press X to look for networks."]);
    for _ in 0..60 {
        r.radio.say(home());
        r.tick(1_000);
    }
    assert_eq!(scans(&r.radio), 0, "coming back scanned");
}

#[test]
fn a_rows_panel_under_network_keeps_what_the_scan_found() {
    let mut r = Rig::new();
    r.on_with(&[("Home", -48, true), ("Café", -60, false)], &[]);
    for row in [Sel::Wifi, Sel::Clock, Sel::Ssh] {
        while r.sel() != row {
            r.press(Btn::Down);
        }
        r.press(Btn::Y);
        assert!(matches!(r.a.phase(), Phase::Info(_)), "{:?}", r.a.phase());
        r.tick(5_000);
        r.press(Btn::B);
        assert_eq!(r.sel(), row, "B came back to another row");
        assert_eq!(r.names(), ["Home", "Café"]);
    }
    assert_eq!(scans(&r.radio), 0);
}

#[test]
fn wifi_switched_off_mid_scan_drops_what_it_finds() {
    let mut r = Rig::new();
    r.on_saved(&[]);
    r.press(Btn::X);
    r.press(Btn::Left);
    assert!(!r.a.wifi().enabled());
    r.radio.say(heard(&[("Home", -48, true)]));
    r.tick(16);
    assert!(!r.a.wifi().scanning(), "the scan's answer did not end it");
    r.press(Btn::Right);
    r.tick(16);
    assert!(r.names().is_empty(), "{:?}", r.names());
    assert_eq!(r.status(), ["Press X to look for networks."]);
}

#[test]
fn a_scan_that_hears_nothing_says_so() {
    let mut r = Rig::new();
    r.on_with(&[], &[]);
    assert!(r.names().is_empty());
    assert_eq!(r.status(), ["No networks found. Press X to look again."]);
}

/// Every note the Network screen says fits the list's panel at the narrowest canvas, with its
/// room in from the well's side either side.
#[test]
fn every_note_fits_the_list() {
    let notes = [
        "This handheld has no Wi-Fi.",
        "Wi-Fi is off.",
        "The Wi-Fi radio did not start.",
        "Looking for networks…",
        "Press X to look for networks.",
        "No networks found. Press X to look again.",
        "Not connected. Pick a network to join.",
        "Waiting for an address…",
        "Wrong passphrase?",
    ];
    let pal = palette(0);
    let well = network_chrome(3, pal).wells[1];
    for note in notes {
        let face = text_face(note, pal.dim);
        assert!(
            face.w + NOTE_IN * 2 <= well[2],
            "{note:?} is {} wide in a well {} wide",
            face.w,
            well[2]
        );
    }
}

#[test]
fn a_new_network_with_a_lock_asks_for_its_passphrase_and_joins_with_it() {
    let mut r = Rig::new();
    r.on_with(&[("Home", -48, true)], &[]);
    r.go_to("Home");
    r.press(Btn::A);
    assert_eq!(r.typing().as_deref(), Some(""), "no keyboard");
    for _ in 0..8 {
        r.press(Btn::A);
    }
    r.press(Btn::Start);
    assert_eq!(r.typing(), None, "the keyboard stayed up");
    assert_eq!(
        r.radio.asked(),
        [WifiJob::Join {
            ssid: b"Home".to_vec(),
            psk: "qqqqqqqq".into(),
            replace: None
        }]
    );
    assert_eq!(r.status(), ["Joining Home…"]);
    assert_eq!(r.sel(), Sel::Network(b"Home".to_vec()));
}

#[test]
fn a_passphrase_too_short_is_refused_without_asking_the_radio() {
    let mut r = Rig::new();
    r.on_with(&[("Home", -48, true)], &[]);
    r.go_to("Home");
    r.press(Btn::A);
    for _ in 0..3 {
        r.press(Btn::A);
    }
    r.press(Btn::Start);
    assert_eq!(r.typing().as_deref(), Some("qqq"));
    assert!(r.a.typing().unwrap().refused, "the field did not say no");
    assert_eq!(r.radio.asked(), []);
    r.press(Btn::Y);
    assert!(
        !r.a.typing().unwrap().refused,
        "typing on left the field refusing"
    );
    assert_eq!(r.typing().as_deref(), Some("qqq "));
}

#[test]
fn the_keyboard_types_capitals_and_symbols_and_deletes() {
    let mut r = Rig::new();
    r.on_with(&[("Home", -48, true)], &[]);
    r.go_to("Home");
    r.press(Btn::A);
    r.press(Btn::X);
    r.press(Btn::A);
    r.press(Btn::X);
    r.press(Btn::A);
    r.press(Btn::R1);
    r.press(Btn::A);
    r.press(Btn::Up);
    r.press(Btn::A);
    assert_eq!(r.typing().as_deref(), Some("Qq`!"));
    r.press(Btn::B);
    assert_eq!(r.typing().as_deref(), Some("Qq`"));
}

#[test]
fn b_on_an_empty_field_and_menu_go_back_to_the_list() {
    let mut r = Rig::new();
    r.on_with(&[("Home", -48, true)], &[]);
    r.go_to("Home");
    r.press(Btn::A);
    r.press(Btn::A);
    r.press(Btn::B);
    assert_eq!(
        r.typing().as_deref(),
        Some(""),
        "B with text left the keyboard"
    );
    r.press(Btn::B);
    assert_eq!(r.typing(), None);
    assert!(
        matches!(r.a.phase(), Phase::Network(_)),
        "B left Wi-Fi altogether"
    );
    r.press(Btn::A);
    r.a.apply(Action::QuickMenu);
    assert_eq!(r.typing(), None);
    assert!(
        matches!(r.a.phase(), Phase::Network(_)),
        "MENU left Wi-Fi altogether"
    );
    assert_eq!(r.radio.asked(), []);
}

#[test]
fn an_open_network_is_joined_without_a_passphrase() {
    let mut r = Rig::new();
    r.on_with(&[("Café", -60, false)], &[]);
    r.go_to("Café");
    r.press(Btn::A);
    assert_eq!(r.typing(), None);
    assert_eq!(
        r.radio.asked(),
        [WifiJob::Join {
            ssid: "Café".as_bytes().to_vec(),
            psk: String::new(),
            replace: None
        }]
    );
}

#[test]
fn a_saved_network_is_joined_by_its_id_and_y_forgets_it() {
    let mut r = Rig::new();
    r.on_with(&[("Home", -48, true)], &[(3, "Home")]);
    r.go_to("Home");
    r.press(Btn::A);
    assert_eq!(r.typing(), None, "a saved network asked for its passphrase");
    assert_eq!(r.radio.asked(), [WifiJob::Select(3)]);
    r.press(Btn::Y);
    assert_eq!(r.radio.asked(), [WifiJob::Select(3), WifiJob::Forget(3)]);
}

#[test]
fn a_saved_network_out_of_reach_is_still_listed_to_be_forgotten() {
    let mut r = Rig::new();
    r.on_with(&[("Home", -48, true)], &[(5, "Cabin")]);
    assert_eq!(r.names(), ["Home", "Cabin"]);
    let cabin = r.view().networks[1].clone();
    assert_eq!(
        cabin,
        NetworkLine {
            name: "Cabin".into(),
            value: Some("SAVED"),
            signal: None,
            lock: false,
        },
        "an absent network showed a signal"
    );
    r.go_to("Cabin");
    assert!(r.a.network_forgets(), "the bar would not offer Y FORGET");
    r.press(Btn::Y);
    assert_eq!(r.radio.asked(), [WifiJob::Forget(5)]);
}

#[test]
fn network_holds_wifi_and_only_with_it_on_network_time_and_ssh() {
    let mut r = Rig::new();
    r.open_wifi();
    assert_eq!(r.settings(), ["WI-FI"]);
    assert_eq!(
        r.view().settings[0],
        SettingLine {
            label: "WI-FI",
            value: Some("OFF"),
            carets: true,
            opens: false,
        }
    );
    assert!(r.names().is_empty(), "networks listed with Wi-Fi off");
    assert_eq!(r.status(), ["Wi-Fi is off."]);
    assert_eq!(r.view().lit, Lit::Setting(0));

    let mut r = Rig::new();
    r.on_with(&[("Home", -48, true)], &[]);
    assert_eq!(r.settings(), ["WI-FI", "NETWORK TIME", "SSH"]);
    let view = r.view();
    assert_eq!(view.settings[0].value, Some("ON"));
    let ntp = if r.a.wifi().ntp() { "ON" } else { "OFF" };
    assert_eq!(view.settings[1].value, Some(ntp), "{:?}", view.settings[1]);
    assert!(view.settings[1].carets && !view.settings[1].opens);
    assert_eq!(view.settings[2].value, Some("ON"), "SSH is not on");
    assert!(view.settings[2].carets && !view.settings[2].opens);

    r.press(Btn::Down);
    r.press(Btn::Down);
    assert_eq!(r.view().lit, Lit::Setting(2), "SSH");
    r.press(Btn::Down);
    assert_eq!(r.view().lit, Lit::Network(0), "the first network");
    assert!(
        !r.a.network_forgets(),
        "Y FORGET offered on a network never saved"
    );
    r.press(Btn::Up);
    r.press(Btn::Up);
    r.press(Btn::Up);
    r.press(Btn::Left);
    assert_eq!(r.settings(), ["WI-FI"], "Wi-Fi off kept its other rows");
}

#[test]
fn the_networks_list_the_joined_one_then_by_signal_then_the_saved_out_of_reach() {
    let mut r = Rig::new();
    r.on_with(
        &[
            ("Upstairs 5G", -45, true),
            ("Café Wi-Fi", -60, false),
            ("Home", -70, true),
            ("Attic", -90, true),
        ],
        &[(1, "Home"), (2, "Cabin")],
    );
    r.radio.say(joined("Home", Some("192.168.1.20")));
    r.tick(16);
    let line = |name: &str, value, signal, lock| NetworkLine {
        name: name.into(),
        value,
        signal,
        lock,
    };
    assert_eq!(
        r.view().networks,
        [
            line("Home", Some("CONNECTED"), Some(2), true),
            line("Upstairs 5G", None, Some(3), true),
            line("Café Wi-Fi", None, Some(3), false),
            line("Attic", None, Some(1), true),
            line("Cabin", Some("SAVED"), None, false),
        ],
        "the name keeps its case, accent and all"
    );
    assert_eq!(
        r.status(),
        Vec::<String>::new(),
        "joined and addressed, a note still said something: SSH has the address"
    );
}

#[test]
fn a_list_longer_than_its_panel_scrolls_a_whole_row_at_a_time() {
    let mut r = Rig::new();
    let nets = many(30);
    let nets: Vec<(&str, i32, bool)> = nets.iter().map(|(s, d, l)| (s.as_str(), *d, *l)).collect();
    r.on_with(&nets, &[]);
    let shown = networks_shown(3, r.status().len());
    assert!(shown < 30, "the panel holds all {shown}");
    assert_eq!(r.names().len(), shown, "the panel is not full");
    assert_eq!(r.names()[0], "Net 00");

    // down to the last network in view: the list stays
    r.go_to(&format!("Net {:02}", shown - 1));
    assert_eq!(r.names()[0], "Net 00");
    assert_eq!(r.view().lit, Lit::Network(shown - 1));
    // one more: the list moves up a row, the cursor on the bottom one
    r.press(Btn::Down);
    assert_eq!(r.names()[0], "Net 01");
    assert_eq!(r.lit_name().as_deref(), Some(&*format!("Net {shown:02}")));
    assert_eq!(r.view().lit, Lit::Network(shown - 1));
    assert_eq!(r.names().len(), shown);
    // and back up: it moves only once the cursor passes its top
    for _ in 0..shown - 1 {
        r.press(Btn::Up);
    }
    assert_eq!(r.names()[0], "Net 01");
    assert_eq!(r.view().lit, Lit::Network(0));
    r.press(Btn::Up);
    assert_eq!(r.names()[0], "Net 00");
    assert_eq!(r.view().lit, Lit::Network(0));

    // up from Wi-Fi goes round to the last network, the list at its end
    r.press(Btn::Up);
    r.press(Btn::Up);
    r.press(Btn::Up);
    r.press(Btn::Up);
    assert_eq!(r.view().lit, Lit::Network(shown - 1));
    assert_eq!(r.lit_name().as_deref(), Some("Net 29"));
    assert_eq!(r.names()[0], format!("Net {:02}", 30 - shown));
    // down from there goes round to Wi-Fi, and the list stays where it was
    r.press(Btn::Down);
    assert_eq!(r.view().lit, Lit::Setting(0));
    assert_eq!(r.names()[0], format!("Net {:02}", 30 - shown));
}

#[test]
fn before_a_scan_it_says_to_press_x_and_while_it_scans_that_it_is_looking() {
    let mut r = Rig::new();
    r.on_saved(&[]);
    assert!(r.names().is_empty());
    assert_eq!(r.status(), ["Press X to look for networks."]);
    r.press(Btn::X);
    assert_eq!(r.status(), ["Looking for networks…"]);
    r.tick(5_000);
    assert_eq!(r.status(), ["Looking for networks…"]);
    r.radio.say(heard(&[("Home", -50, true)]));
    r.tick(16);
    assert_eq!(r.status(), ["Not connected. Pick a network to join."]);
}

#[test]
fn a_long_name_is_cut_short_in_a_note() {
    let mut r = Rig::new();
    let long = "The network with the very long name upstairs";
    r.on_with(&[(long, -60, false)], &[]);
    r.go_to(long);
    r.press(Btn::A);
    r.tick(JOIN_TIMEOUT_MS * 2);
    let notes = r.status();
    assert_eq!(notes.len(), 1);
    assert!(notes[0].starts_with("Could not join The network with") && notes[0].ends_with("…."));
    assert!(
        notes[0].chars().count() <= 43,
        "{:?} is wider than the list's panel at 640x480",
        notes[0]
    );
    assert_eq!(r.names(), [long], "the list cut the name; its row cuts it");
}

#[test]
fn a_passphrase_turned_away_says_so_and_is_asked_for_again() {
    let mut r = Rig::new();
    r.on_with(&[("Home", -48, true)], &[]);
    r.go_to("Home");
    r.press(Btn::A);
    for _ in 0..8 {
        r.press(Btn::A);
    }
    r.press(Btn::Start);
    r.radio.say(WifiNews::Saved(vec![Saved {
        id: 0,
        ssid: b"Home".to_vec(),
        current: false,
        refused: true,
    }]));
    r.tick(16);
    assert_eq!(r.status(), ["Could not join Home.", "Wrong passphrase?"]);
    r.radio.forget_asked();
    r.press(Btn::A);
    assert_eq!(
        r.typing().as_deref(),
        Some(""),
        "a turned-away network was retried as it was"
    );
    assert_eq!(r.a.network_typing().unwrap().replace, Some(0));
    assert_eq!(r.radio.asked(), []);
}

#[test]
fn a_join_that_never_finishes_gives_up() {
    let mut r = Rig::new();
    r.on_with(&[("Café", -60, false)], &[]);
    r.go_to("Café");
    r.press(Btn::A);
    r.tick(JOIN_TIMEOUT_MS / 2);
    assert_eq!(r.status(), ["Joining Café…"]);
    r.tick(JOIN_TIMEOUT_MS);
    assert_eq!(r.status(), ["Could not join Café."]);
}

#[test]
fn joined_it_waits_for_an_address_then_leaves_it_to_ssh() {
    let mut r = Rig::new();
    r.on_with(&[("Home", -48, true)], &[(0, "Home")]);
    r.radio.say(joined("Home", None));
    r.tick(16);
    assert_eq!(r.status(), ["Waiting for an address…"]);
    assert_eq!(r.view().networks[0].value, Some("CONNECTED"));
    r.radio.say(joined("Home", Some("192.168.1.20")));
    r.tick(16);
    assert!(
        r.status().is_empty(),
        "the address is SSH's to say: {:?}",
        r.status()
    );
    assert_eq!(r.a.wifi_summary(), Summary::Joined("Home".into()));
}

#[test]
fn the_top_bar_lights_wifi_and_ssh_once_a_network_is_joined() {
    let mut r = Rig::new();
    assert!(
        !r.a.links().wifi && !r.a.links().ssh,
        "lit before Wi-Fi is on"
    );
    r.on_with(&[("Home", -48, true)], &[(0, "Home")]);
    assert!(!r.a.links().wifi, "lit with Wi-Fi on but nothing joined");
    r.radio.say(joined("Home", Some("192.168.1.20")));
    r.tick(16);
    assert!(r.a.links().wifi, "Wi-Fi is not lit once joined");
    assert!(
        r.a.links().ssh,
        "ssh is not lit once the handheld can be reached"
    );
    assert_eq!(
        r.a.links().bluetooth,
        Some(false),
        "Bluetooth is not shown off"
    );
}

#[test]
fn the_cursor_stays_on_its_network_when_a_scan_reorders_the_list() {
    let mut r = Rig::new();
    r.on_with(&[("Near", -40, true), ("Far", -80, true)], &[]);
    r.go_to("Far");
    r.radio.say(WifiNews::Seen(vec![
        Seen {
            ssid: b"Far".to_vec(),
            rssi: -45,
            secured: true,
        },
        Seen {
            ssid: b"Near".to_vec(),
            rssi: -70,
            secured: true,
        },
    ]));
    r.tick(16);
    assert_eq!(r.lit_name().as_deref(), Some("Far"));
    assert_eq!(r.names(), ["Far", "Near"]);
    r.press(Btn::A);
    assert!(
        r.a.network_typing().is_some_and(|t| t.ssid == b"Far"),
        "A picked another network"
    );
}

#[test]
fn settings_says_off_on_or_the_network_joined() {
    let mut r = Rig::new();
    assert_eq!(r.a.wifi_summary(), Summary::Off);
    r.on_with(&[], &[]);
    assert_eq!(r.a.wifi_summary(), Summary::On);
    r.radio.say(joined("Home", Some("10.0.0.2")));
    r.tick(16);
    assert_eq!(r.a.wifi_summary(), Summary::Joined("Home".into()));
}

#[test]
fn network_time_switches_on_the_clock_row() {
    let mut r = Rig::new();
    r.on_with(&[], &[]);
    r.press(Btn::Down);
    assert_eq!(r.sel(), Sel::Clock);
    let was = r.a.wifi().ntp();
    r.press(Btn::Right);
    assert_eq!(r.radio.asked(), [WifiJob::Clock(!was)]);
    assert_eq!(r.a.wifi().ntp(), !was);
}

#[test]
fn a_radio_that_never_appears_says_so() {
    let mut r = Rig::new();
    r.open_wifi();
    r.press(Btn::A);
    r.radio.say(WifiNews::Started(false));
    r.tick(16);
    assert_eq!(r.status(), ["The Wi-Fi radio did not start."]);
}

#[test]
fn the_shelf_glances_at_the_link_and_asks_its_status_only_when_a_glance_disagrees() {
    let mut r = Rig::with(
        AppState {
            wifi: true,
            ..AppState::default()
        },
        Radio::default(),
    );
    assert!(matches!(r.a.phase(), Phase::Shelf), "{:?}", r.a.phase());
    r.radio.say(joined("Home", Some("192.168.1.20")));
    r.radio.say(WifiNews::Saved(Vec::new()));
    r.tick(16);
    assert_eq!(r.radio.asked(), [WifiJob::Up, WifiJob::Glance]);
    r.radio.forget_asked();
    // a minute of glances that find the link as the status says
    for _ in 0..60 {
        r.radio.say(WifiNews::Glanced(true));
        r.tick(16);
        r.tick(GLANCE_MS);
    }
    let asked = r.radio.asked();
    assert_eq!(
        asked,
        vec![WifiJob::Glance; 60],
        "the shelf asked for more than a glance a second"
    );
    r.radio.forget_asked();
    r.radio.say(WifiNews::Glanced(false));
    r.tick(16);
    assert_eq!(
        r.radio.asked(),
        [WifiJob::Refresh],
        "a glance that found the link down did not ask for the status"
    );
    // a status that goes on saying joined is asked no more often than the menus ask it
    let mut refreshes = 0;
    for _ in 0..10 {
        for job in r.radio.asked() {
            match job {
                WifiJob::Glance => r.radio.say(WifiNews::Glanced(false)),
                WifiJob::Refresh => {
                    refreshes += 1;
                    r.radio.say(joined("Home", Some("192.168.1.20")));
                    r.radio.say(WifiNews::Saved(Vec::new()));
                }
                other => panic!("the shelf asked for {other:?}"),
            }
        }
        r.radio.forget_asked();
        r.tick(16);
        r.tick(GLANCE_MS);
    }
    assert!(
        (3..=6).contains(&refreshes),
        "{refreshes} statuses in ten seconds of glances that disagree"
    );
}

/// The panel Y opened, with what it says.
fn info(r: &Rig) -> InfoView {
    r.a.info_view().expect("no panel is open")
}

/// A status with Home joined or not, and network time on, set by a server or not.
fn timed(joined: bool, synced: bool) -> WifiNews {
    WifiNews::Status(Status {
        enabled: true,
        state: if joined { "COMPLETED" } else { "SCANNING" }.into(),
        ssid: joined.then(|| b"Home".to_vec()),
        ip: joined.then(|| "192.168.1.20".into()),
        rssi: joined.then_some(-50),
        mac: Some("e4:5f:01:2a:9c:33".into()),
        hostname: "rg35xxsp".into(),
        ntp: true,
        ntp_synced: synced,
    })
}

#[test]
fn y_on_each_of_networks_rows_opens_its_panel_and_switches_nothing() {
    let mut r = Rig::new();
    r.on_saved(&[(0, "Home")]);
    r.radio.say(joined("Home", Some("192.168.1.20")));
    r.tick(16);
    for (row, title) in [
        (Sel::Wifi, "WI-FI"),
        (Sel::Clock, "NETWORK TIME"),
        (Sel::Ssh, "SSH"),
    ] {
        while r.sel() != row {
            r.press(Btn::Down);
        }
        let was = (r.a.wifi().enabled(), r.a.wifi().ntp(), r.a.ssh_on());
        r.radio.forget_asked();
        r.press(Btn::Y);
        assert!(matches!(r.a.phase(), Phase::Info(_)), "{:?}", r.a.phase());
        assert_eq!(info(&r).title, title);
        r.press(Btn::B);
        assert_eq!(r.sel(), row, "B came back to another row");
        assert_eq!(
            (r.a.wifi().enabled(), r.a.wifi().ntp(), r.a.ssh_on()),
            was,
            "Y on {row:?} switched something"
        );
        let switched = r.radio.asked().into_iter().find(|job| {
            matches!(
                job,
                WifiJob::Up | WifiJob::Down | WifiJob::Clock(_) | WifiJob::Forget(_)
            )
        });
        assert_eq!(switched, None, "Y on {row:?} asked the radio");
    }
}

#[test]
fn wifi_says_the_network_joined_and_how_the_handheld_is_known_on_it() {
    let mut r = Rig::new();
    r.on_saved(&[(0, "Home")]);
    r.radio.say(joined("Home", Some("192.168.1.20")));
    r.tick(16);
    r.press(Btn::Y);
    let line = |label, value: &str| (label, value.to_string());
    assert_eq!(
        info(&r),
        InfoView {
            title: "WI-FI",
            lines: vec![
                line("NETWORK", "Home"),
                line("SIGNAL", "-50 dBm"),
                line("IP", "192.168.1.20"),
                line("HOST", "rg35xxsp.local"),
                line("MAC", "e4:5f:01:2a:9c:33"),
            ],
            note: "Networks: Config/wifi.conf",
        }
    );
}

#[test]
fn wifi_says_what_comes_first_until_the_handheld_has_an_address() {
    let mut r = Rig::new();
    r.open_wifi();
    assert!(!r.a.wifi().enabled());
    let mut notes = Vec::new();
    let mut note = |r: &mut Rig| {
        r.press(Btn::Y);
        let view = info(r);
        assert!(view.lines.is_empty(), "{view:?}");
        notes.push(view.note);
        r.press(Btn::B);
    };
    note(&mut r);
    r.press(Btn::A);
    note(&mut r);
    r.radio.say(joined("Home", None));
    r.tick(16);
    note(&mut r);
    assert_eq!(
        notes,
        [
            "Turn Wi-Fi on first.",
            "Not connected to a network.",
            "Waiting for an address…",
        ]
    );
    let none = Wifi::new(Box::new(Radio::absent()));
    assert_eq!(wifi_view(&none).note, "This handheld has no Wi-Fi.");
}

#[test]
fn a_long_network_name_is_cut_to_fit_its_line() {
    let long = "The Long Network Name Of Upstairs";
    let mut r = Rig::new();
    r.on_saved(&[(0, long)]);
    r.radio.say(joined(long, Some("192.168.1.20")));
    r.tick(16);
    r.press(Btn::Y);
    let (label, name) = info(&r).lines[0].clone();
    assert_eq!(label, "NETWORK");
    assert_eq!(name, "The Long Network Name…");
    assert_eq!(name.chars().count(), NETWORK_SHOWN);
    // the row's inside less its name and the room the name keeps (kit/row.rs)
    let pal = palette(0);
    let well = info_chrome("WI-FI", 5, pal).well;
    let room = row_width(well[2]) - 2 * (1 + 4) - 6 - text_face("NETWORK", pal.dim).w;
    let longest = "W".repeat(NETWORK_SHOWN);
    assert!(
        text_face(&longest, pal.dim).w <= room + 1,
        "{NETWORK_SHOWN} letters are wider than the {room} pixels the line has"
    );
}

#[test]
fn network_time_says_whether_a_server_set_the_clock_and_what_it_shows() {
    let state = AppState {
        wifi: true,
        utc_offset_min: -180,
        ..AppState::default()
    };
    let mut r = Rig::with(state, Radio::default());
    r.on_saved(&[(0, "Home")]);
    r.radio.say(timed(true, true));
    r.tick(16);
    r.press(Btn::Down);
    assert_eq!(r.sel(), Sel::Clock);
    r.press(Btn::Y);
    let line = |label, value: &str| (label, value.to_string());
    // the test clock: 2026-08-12 20:53 UTC, three hours behind
    assert_eq!(
        info(&r),
        InfoView {
            title: "NETWORK TIME",
            lines: vec![
                line("STATUS", "SYNCED"),
                line("DATE", "2026-08-12"),
                line("TIME", "17:53"),
                line("ZONE", "UTC-03:00"),
            ],
            note: "Set the zone in Date & Time",
        }
    );
    r.radio.say(timed(true, false));
    r.tick(16);
    let view = info(&r);
    assert_eq!(view.lines[0], line("STATUS", "WAITING"));
    assert_eq!(view.note, "Waiting for a time server…");
    r.radio.say(timed(false, false));
    r.tick(16);
    let view = info(&r);
    assert_eq!(view.lines[0], line("STATUS", "WAITING"));
    assert_eq!(view.note, "Join a Wi-Fi network first.");
    r.press(Btn::B);
    r.press(Btn::Left);
    assert!(!r.a.wifi().ntp(), "Left did not switch network time off");
    r.press(Btn::Y);
    let view = info(&r);
    assert!(view.lines.is_empty(), "{view:?}");
    assert_eq!(view.note, "Turn Network Time on first.");
}

#[test]
fn every_note_of_wifis_and_network_times_panels_fits() {
    let notes = [
        "This handheld has no Wi-Fi.",
        "Turn Wi-Fi on first.",
        "Not connected to a network.",
        "Waiting for an address…",
        "Networks: Config/wifi.conf",
        "Turn Network Time on first.",
        "Set the zone in Date & Time",
        "Waiting for a time server…",
        "Join a Wi-Fi network first.",
    ];
    let pal = palette(0);
    for lines in [0, 4, 5] {
        let well = info_chrome("WI-FI", lines, pal).well;
        for note in notes {
            let face = text_face(note, pal.dim);
            assert!(
                face.w + NOTE_IN * 2 <= well[2],
                "{note:?} is {} wide in a well {} wide",
                face.w,
                well[2]
            );
        }
    }
}
