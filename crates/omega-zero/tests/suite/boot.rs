use crate::common;

use common::{boot, tmp_root_with_carts};
use omega_input::{Action, Btn};
use omega_store::{read_app_state, write_app_state, AppState, Platform};
use omega_zero::app::{App, Phase};

fn seated(cart: &str) -> AppState {
    AppState {
        cart: Some(cart.into()),
        clock_set: true,
        utc_offset_min: 0,
        ..Default::default()
    }
}

#[test]
fn boot_with_a_seated_cart_never_shows_the_shelf() {
    let d = tmp_root_with_carts(&["Emerald"]);
    write_app_state(d.path(), &seated("Emerald")).unwrap();
    let a = App::boot(d.path());
    assert!(
        matches!(a.phase(), Phase::Inserting { .. }),
        "boot must go straight to the seated cart, not the shelf"
    );
}

#[test]
fn boot_with_an_empty_slot_shows_the_shelf() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let a = boot(d.path());
    assert!(matches!(a.phase(), Phase::Shelf));
}

#[test]
fn boot_with_a_cart_that_no_longer_exists_falls_back_to_the_shelf() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    write_app_state(d.path(), &seated("Deleted")).unwrap();
    let a = App::boot(d.path());
    assert!(matches!(a.phase(), Phase::Shelf));
}

#[test]
fn boot_leaves_the_shelf_sitting_on_the_resumed_cart() {
    let d = tmp_root_with_carts(&["Advance Wars", "Emerald", "Fire Emblem"]);
    write_app_state(d.path(), &seated("Emerald")).unwrap();
    let mut a = App::boot(d.path());
    a.on_core_ready();
    for _ in 0..120 {
        a.update(1.0 / 60.0);
    }
    a.apply(Action::Eject);
    for _ in 0..120 {
        a.update(1.0 / 60.0);
    }
    assert!(matches!(a.phase(), Phase::Shelf));
    a.apply(Action::Insert);
    let Phase::Inserting { cart, .. } = a.phase() else {
        panic!("insert after eject did nothing: {:?}", a.phase())
    };
    assert_eq!(cart, "Emerald");
}

#[test]
fn a_seated_cart_is_recorded_so_the_next_boot_can_resume_it() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let mut a = boot(d.path());
    a.apply(Action::Insert);
    a.on_core_ready();
    assert_eq!(
        read_app_state(d.path()).cart,
        None,
        "a cart that has not seated yet is not in the slot"
    );
    for _ in 0..120 {
        a.update(1.0 / 60.0);
    }
    assert!(matches!(a.phase(), Phase::Playing { .. }));
    assert_eq!(read_app_state(d.path()).cart, Some("Emerald".into()));
}

#[test]
fn a_cart_that_is_gone_takes_its_shelf_out_of_the_slot_with_it() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    write_app_state(
        d.path(),
        &AppState {
            cart: Some("Deleted".into()),
            cart_platform: Some(Platform::Gbc),
            clock_set: true,
            ..Default::default()
        },
    )
    .unwrap();
    let mut a = App::boot(d.path());
    a.apply(Action::MuteToggle);
    let s = read_app_state(d.path());
    assert_eq!(s.cart, None);
    assert_eq!(
        s.cart_platform, None,
        "the card names a shelf beside a line that names no cart"
    );
}

#[test]
fn a_cart_that_fails_to_load_leaves_the_slot_empty() {
    let d = tmp_root_with_carts(&["Emerald"]);
    write_app_state(d.path(), &seated("Emerald")).unwrap();
    let mut a = App::boot(d.path());
    a.on_core_failed();
    for _ in 0..120 {
        a.update(1.0 / 60.0);
    }
    assert!(matches!(a.phase(), Phase::Shelf));
    assert_eq!(read_app_state(d.path()).cart, None);
}

#[test]
fn seating_a_cart_preserves_the_levels_already_in_the_file() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    write_app_state(
        d.path(),
        &AppState {
            cart: None,
            brightness: 2,
            blue_light: 7,
            volume: 35,
            muted: false,
            clock_set: true,
            utc_offset_min: 0,
            ..AppState::default()
        },
    )
    .unwrap();
    let mut a = App::boot(d.path());
    a.apply(Action::Insert);
    a.on_core_ready();
    for _ in 0..120 {
        a.update(1.0 / 60.0);
    }
    let s = read_app_state(d.path());
    assert_eq!((s.brightness, s.blue_light, s.volume), (2, 7, 35));
}

#[test]
fn a_resume_draws_no_shelf_but_a_chosen_insert_does() {
    let seated = || {
        let d = common::tmp_root_with_carts(&["Emerald", "Fusion", "Wars"]);
        write_app_state(
            d.path(),
            &AppState {
                cart: Some("Emerald".into()),
                clock_set: true,
                utc_offset_min: 0,
                ..Default::default()
            },
        )
        .unwrap();
        (App::boot(d.path()), d)
    };
    let chosen = || {
        let d = common::tmp_root_with_carts(&["Emerald", "Fusion", "Wars"]);
        write_app_state(
            d.path(),
            &AppState {
                clock_set: true,
                utc_offset_min: 0,
                ..Default::default()
            },
        )
        .unwrap();
        let mut a = App::boot(d.path());
        a.apply(Action::Insert);
        (a, d)
    };

    let (resume, _d1) = seated();
    let (pick, _d2) = chosen();
    let (r, p) = (draw_count(&resume), draw_count(&pick));
    assert!(
        p > r,
        "a resume drew {r} and a chosen insert {p}: the shelf is on screen for both"
    );

    let (mut resume, _d3) = seated();
    for frame in 0..90 {
        assert!(
            draw_count(&resume) <= r,
            "frame {frame}: the shelf appeared partway through a resume"
        );
        resume.update(1.0 / 60.0);
    }
}

fn draw_count(a: &App) -> usize {
    let mut out = Vec::new();
    a.draw(&mut out);
    out.len()
}

#[test]
fn a_loose_card_shows_an_empty_shelf_and_nothing_on_it_is_moved() {
    let d = tempfile::tempdir().unwrap();
    for sub in ["Games", "Saves", "Labels", "States"] {
        std::fs::create_dir_all(d.path().join(sub)).unwrap();
    }
    std::fs::write(d.path().join("Games/Emerald.gba"), vec![0u8; 0x100]).unwrap();
    std::fs::write(d.path().join("Saves/Emerald.sav"), vec![7u8; 0x10000]).unwrap();
    std::fs::write(d.path().join("Labels/Emerald.png"), b"png").unwrap();
    let old_states = d.path().join("States/mgba/Emerald");
    std::fs::create_dir_all(&old_states).unwrap();
    std::fs::write(old_states.join("resume.state"), b"resume").unwrap();

    let a = App::boot(d.path());

    assert_eq!(
        a.carts().count(),
        0,
        "a loose rom reached the shelf, so something is still reading outside Games/<platform>/"
    );
    assert_eq!(
        std::fs::read(d.path().join("Games/Emerald.gba"))
            .unwrap()
            .len(),
        0x100,
        "the loose rom was moved or disturbed"
    );
    assert_eq!(
        std::fs::read(d.path().join("Saves/Emerald.sav"))
            .unwrap()
            .len(),
        0x10000,
        "the loose battery save was moved or disturbed"
    );
    assert_eq!(
        std::fs::read(d.path().join("Labels/Emerald.png")).unwrap(),
        b"png",
        "the loose label was moved"
    );
    assert_eq!(
        std::fs::read(old_states.join("resume.state")).unwrap(),
        b"resume",
        "a pre-namespacing state directory was moved"
    );
}

#[test]
fn boot_creates_the_folders_a_person_has_to_file_into() {
    let d = tempfile::tempdir().unwrap();

    App::boot(d.path());

    for name in omega_zero::root::DIRS {
        assert!(
            d.path().join(name).is_dir(),
            "{name} is missing, so nothing on the card says where its files go"
        );
    }
}

fn two_tetrises() -> tempfile::TempDir {
    let d = tmp_root_with_carts(&["Tetris", "Emerald"]);
    common::write_gb_cart(&d, "Tetris", "TETRIS");
    d
}

fn resumed(d: &tempfile::TempDir, platform: Option<Platform>) -> App {
    write_app_state(
        d.path(),
        &AppState {
            cart: Some("Tetris".into()),
            cart_platform: platform,
            clock_set: true,
            utc_offset_min: 0,
            ..Default::default()
        },
    )
    .unwrap();
    App::boot(d.path())
}

#[test]
fn the_platform_on_the_card_decides_which_tetris_comes_back() {
    let d = two_tetrises();
    for platform in [Platform::Gb, Platform::Gba] {
        let a = resumed(&d, Some(platform));
        let cart = a.seated_cart().expect("nothing seated");
        assert_eq!(
            cart.platform, platform,
            "the card named {platform:?} and a {:?} cart came back",
            cart.platform
        );
        assert!(
            cart.rom.ends_with(format!(
                "{}/Tetris.{}",
                platform.dir_name(),
                platform.extensions()[0]
            )),
            "the rom the slot is holding is {:?}",
            cart.rom
        );
    }
}

#[test]
fn a_card_that_never_said_resumes_the_gba_cart() {
    let d = two_tetrises();
    let a = resumed(&d, None);
    assert_eq!(
        a.seated_cart().expect("nothing seated").platform,
        Platform::Gba
    );
}

#[test]
fn a_named_shelf_that_no_longer_has_the_cart_is_an_empty_slot() {
    let d = two_tetrises();
    let a = resumed(&d, Some(Platform::Gbc));
    assert!(
        matches!(a.phase(), Phase::Shelf),
        "a Colour Tetris that is not on the card seated something: {:?}",
        a.phase()
    );
}

#[test]
fn seating_a_cart_records_the_shelf_it_came_off() {
    let d = two_tetrises();
    let mut a = boot(d.path());
    a.apply(Action::GbaDown(Btn::R1));
    assert_eq!(
        a.selected_stem(),
        Some("Tetris"),
        "the shoulder did not ring to the Game Boy shelf"
    );
    a.apply(Action::Insert);
    a.on_core_ready();
    for _ in 0..120 {
        a.update(1.0 / 60.0);
    }
    assert!(matches!(a.phase(), Phase::Playing { .. }));
    let s = read_app_state(d.path());
    assert_eq!(
        (s.cart.as_deref(), s.cart_platform),
        (Some("Tetris"), Some(Platform::Gb)),
        "the card does not say which Tetris is in the slot"
    );

    let again = App::boot(d.path());
    assert_eq!(
        again.seated_cart().expect("nothing seated").platform,
        Platform::Gb
    );
}

#[test]
fn an_eject_forgets_the_platform_with_the_cart() {
    let d = two_tetrises();
    let mut a = resumed(&d, Some(Platform::Gb));
    a.on_core_ready();
    for _ in 0..120 {
        a.update(1.0 / 60.0);
    }
    a.apply(Action::Eject);
    for _ in 0..120 {
        a.update(1.0 / 60.0);
    }
    assert!(matches!(a.phase(), Phase::Shelf));
    let s = read_app_state(d.path());
    assert_eq!((s.cart, s.cart_platform), (None, None));
}

#[test]
fn a_resumed_cart_starts_seated() {
    let d = common::tmp_root_with_carts(&["Emerald", "Fusion"]);
    write_app_state(
        d.path(),
        &AppState {
            cart: Some("Emerald".into()),
            clock_set: true,
            utc_offset_min: 0,
            ..Default::default()
        },
    )
    .unwrap();
    let a = App::boot(d.path());
    assert_eq!(a.seat(), 1.0, "the resumed cart is still sliding in");
}

/// A game whose core took omega-zero down as it opened is noted on the card: the next boot
/// leaves it on the shelf, selected, rather than open it again and go down with it each time.
#[test]
fn a_cart_whose_start_took_omega_zero_down_is_left_on_the_shelf() {
    let d = tmp_root_with_carts(&["Advance Wars", "Emerald", "Fire Emblem"]);
    write_app_state(d.path(), &seated("Emerald")).unwrap();
    let note = d.path().join("Config/.starting");
    std::fs::write(&note, "GBA/Emerald").unwrap();

    let a = App::boot(d.path());
    assert!(matches!(a.phase(), Phase::Shelf), "{:?}", a.phase());
    assert_eq!(a.selected_stem(), Some("Emerald"));
    assert!(!note.exists(), "the note outlived the boot that read it");

    let a = App::boot(d.path());
    assert!(
        matches!(a.phase(), Phase::Inserting { .. }),
        "a boot after that one does not try the game again"
    );
}

#[test]
fn a_core_is_noted_as_opening_until_it_has_run_a_moment() {
    let d = common::tmp_root_with_real_carts(&["Emerald", "Fusion"]);
    common::clocked(d.path());
    let note = d.path().join("Config/.starting");
    let mut s = omega_zero::session::Session::boot(d.path().to_path_buf());
    s.feed(
        [
            omega_input::RawEvent::Down(Btn::A),
            omega_input::RawEvent::Up(Btn::A),
        ],
        16,
    );
    s.update(1.0 / 60.0);
    assert_eq!(
        std::fs::read_to_string(&note).ok().as_deref(),
        Some("GBA/Emerald"),
        "the card does not say which core is opening"
    );
    let began = std::time::Instant::now();
    while note.exists() {
        assert!(
            began.elapsed() < std::time::Duration::from_secs(10),
            "the note stayed though the game ran"
        );
        s.update(1.0 / 60.0);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn a_session_put_away_while_its_core_opens_leaves_no_note() {
    let d = common::tmp_root_with_real_carts(&["Emerald", "Fusion"]);
    common::clocked(d.path());
    let note = d.path().join("Config/.starting");
    let mut s = omega_zero::session::Session::boot(d.path().to_path_buf());
    s.feed(
        [
            omega_input::RawEvent::Down(Btn::A),
            omega_input::RawEvent::Up(Btn::A),
        ],
        16,
    );
    s.update(1.0 / 60.0);
    assert!(note.exists(), "the core opened without a note");
    drop(s);
    assert!(!note.exists(), "a clean power-off was taken for a crash");
}

/// A card with GBA carts and, on a shelf after GBA's, SNES carts.
fn two_shelves() -> tempfile::TempDir {
    let d = tmp_root_with_carts(&[
        "Advance Wars",
        "Emerald",
        "Mario Kart - Super Circuit (USA) (Wii U Virtual Console)",
    ]);
    let snes = d.path().join("Games").join(Platform::Snes.dir_name());
    std::fs::create_dir_all(&snes).unwrap();
    for stem in ["Chrono Trigger", "EarthBound", "Super Metroid"] {
        std::fs::write(snes.join(format!("{stem}.sfc")), vec![0u8; 0x8000]).unwrap();
    }
    d
}

fn shelf_on(a: &App) -> (Option<&'static str>, Option<&str>) {
    (a.shelf_platform_name(), a.selected_stem())
}

#[test]
fn an_empty_slot_boots_onto_the_cart_the_shelf_was_left_on_on_any_shelf() {
    let d = two_shelves();
    write_app_state(
        d.path(),
        &AppState {
            shelf_cart: Some("EarthBound".into()),
            shelf_platform: Some(Platform::Snes),
            clock_set: true,
            ..Default::default()
        },
    )
    .unwrap();
    let a = App::boot(d.path());
    assert!(matches!(a.phase(), Phase::Shelf));
    assert_eq!(shelf_on(&a), (Some("Super Nintendo"), Some("EarthBound")));
}

#[test]
fn a_remembered_cart_that_is_gone_leaves_the_shelf_where_it_was() {
    let d = two_shelves();
    write_app_state(
        d.path(),
        &AppState {
            shelf_cart: Some("Deleted".into()),
            shelf_platform: Some(Platform::Snes),
            clock_set: true,
            ..Default::default()
        },
    )
    .unwrap();
    let a = App::boot(d.path());
    assert_eq!(
        shelf_on(&a),
        (Some("Game Boy Advance"), Some("Advance Wars"))
    );
}

#[test]
fn a_seated_cart_on_a_later_shelf_resumes_with_the_shelf_on_it() {
    let d = two_shelves();
    write_app_state(
        d.path(),
        &AppState {
            cart: Some("Super Metroid".into()),
            cart_platform: Some(Platform::Snes),
            clock_set: true,
            ..Default::default()
        },
    )
    .unwrap();
    let a = App::boot(d.path());
    let Phase::Inserting { cart, .. } = a.phase() else {
        panic!("the seated cart was not resumed: {:?}", a.phase())
    };
    assert_eq!(cart, "Super Metroid");
    assert_eq!(
        shelf_on(&a),
        (Some("Super Nintendo"), Some("Super Metroid"))
    );
}

#[test]
fn the_cart_in_the_slot_named_on_the_card_resumes_by_its_full_name() {
    let d = two_shelves();
    let stem = "Mario Kart - Super Circuit (USA) (Wii U Virtual Console)";
    std::fs::write(
        d.path().join("Config/omega-zero.state"),
        format!("cart={stem}\ncart_platform=gba\nbrightness=5\nblue_light=0\nvolume=60\nmuted=0\nclock_set=1\nutc_offset_min=0\n"),
    )
    .unwrap();
    let a = App::boot(d.path());
    let Phase::Inserting { cart, .. } = a.phase() else {
        panic!("the seated cart was not resumed: {:?}", a.phase())
    };
    assert_eq!(cart, stem);
    assert_eq!(shelf_on(&a), (Some("Game Boy Advance"), Some(stem)));
}

#[test]
fn after_an_eject_and_a_crash_the_shelf_opens_on_the_cart_played_last() {
    let d = two_shelves();
    write_app_state(
        d.path(),
        &AppState {
            clock_set: true,
            ..Default::default()
        },
    )
    .unwrap();
    let mut a = App::boot(d.path());
    a.apply(Action::GbaDown(Btn::R1));
    a.apply(Action::GbaUp(Btn::R1));
    a.apply(Action::ShelfRight);
    assert_eq!(shelf_on(&a), (Some("Super Nintendo"), Some("EarthBound")));
    a.apply(Action::Insert);
    a.on_core_ready();
    for _ in 0..120 {
        a.update(1.0 / 60.0);
    }
    a.apply(Action::Eject);
    for _ in 0..120 {
        a.update(1.0 / 60.0);
    }
    assert!(matches!(a.phase(), Phase::Shelf));
    assert_eq!(read_app_state(d.path()).cart, None);
    // omega-zero is killed here: nothing more is written
    drop(a);
    let a = App::boot(d.path());
    assert!(matches!(a.phase(), Phase::Shelf));
    assert_eq!(shelf_on(&a), (Some("Super Nintendo"), Some("EarthBound")));
}

#[test]
fn powering_off_on_the_shelf_keeps_the_cart_it_was_on() {
    let d = two_shelves();
    write_app_state(
        d.path(),
        &AppState {
            clock_set: true,
            ..Default::default()
        },
    )
    .unwrap();
    let mut a = App::boot(d.path());
    a.apply(Action::GbaDown(Btn::R1));
    a.apply(Action::GbaUp(Btn::R1));
    a.apply(Action::ShelfRight);
    a.apply(Action::ShelfRight);
    assert_eq!(
        shelf_on(&a),
        (Some("Super Nintendo"), Some("Super Metroid"))
    );
    // no platform to take the handheld down: this writes what it would before it does
    a.poweroff();
    let a = App::boot(d.path());
    assert_eq!(
        shelf_on(&a),
        (Some("Super Nintendo"), Some("Super Metroid"))
    );
}
