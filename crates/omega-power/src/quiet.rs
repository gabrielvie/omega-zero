//! Switches off hardware omega-zero never uses, once per session, in the background.
//!
//! Ported from slot.advance (suprsomething). The six USB host controllers (`ohci1-3`,
//! `ehci1-3`) have nothing wired to them on the RG SP; slot.advance measured them as the
//! largest single saving while suspended. The USB-C port has its own controller
//! (`5100000.udc`, with `ehci0` and `ohci0`), which is never touched, so charging and ADB work
//! as before.
//!
//! slot.advance also unloads the Wi-Fi driver here and discards the radio's stale SDIO card.
//! omega-zero does not: its Network setting can switch Wi-Fi on at any moment of a session,
//! and nothing on the card or in slot.advance shows how BaseOS's start-up loads the driver
//! again (slot.advance never does; it needs a restart for `wifi=on`). Unloading it would leave
//! Network, SSH, label downloads and link play without a radio until the next boot.
//!
//! A failure is logged and changes nothing else: hardware sleep never depends on this.
//! Nothing here outlives the session; BaseOS restores the controllers at boot.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// OHCI first, then EHCI, the order that was tested on the device.
pub const IDLE_CONTROLLERS: [(&str, &str); 6] = [
    ("5200000.ohci1-controller", "ohci_enable"),
    ("5310000.ohci2-controller", "ohci_enable"),
    ("5311000.ohci3-controller", "ohci_enable"),
    ("5200000.ehci1-controller", "ehci_enable"),
    ("5310000.ehci2-controller", "ehci_enable"),
    ("5311000.ehci3-controller", "ehci_enable"),
];

pub struct Quiet {
    pub sys: PathBuf,
}

impl Quiet {
    pub fn device() -> Quiet {
        Quiet {
            sys: PathBuf::from("/sys"),
        }
    }

    /// Switches off what it can, logging each step.
    pub fn run(&self) -> QuietReport {
        let usb = self.usb_off();
        eprintln!("omega-zero: quiet: wifi left as BaseOS started it");
        QuietReport { usb }
    }

    /// Returns how many controllers are now off.
    pub fn usb_off(&self) -> usize {
        let soc = self.sys.join("devices/platform/soc");
        let mut off = 0;
        for (dev, file) in IDLE_CONTROLLERS {
            let switch = soc.join(dev).join(file);
            let name = dev
                .split('.')
                .nth(1)
                .unwrap_or(dev)
                .trim_end_matches("-controller");
            match controller_on(&switch) {
                None => {
                    eprintln!("omega-zero: quiet: usb {name}: no switch, left alone");
                    continue;
                }
                Some(false) => {
                    off += 1;
                    continue;
                }
                Some(true) => {}
            }
            if let Err(e) = fs::write(&switch, "0") {
                eprintln!("omega-zero: quiet: usb {name}: {e}");
                continue;
            }
            std::thread::sleep(Duration::from_millis(100));
            match controller_on(&switch) {
                Some(false) => off += 1,
                _ => eprintln!("omega-zero: quiet: usb {name}: did not switch off"),
            }
        }
        eprintln!(
            "omega-zero: quiet: usb host controllers off: {off} of {}",
            IDLE_CONTROLLERS.len()
        );
        off
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct QuietReport {
    pub usb: usize,
}

/// Starts the quiet job on its own thread.
pub fn quiet_hardware(quiet: Quiet) {
    let spawned = std::thread::Builder::new()
        .name("omega-quiet".into())
        .spawn(move || {
            quiet.run();
        });
    if let Err(e) = spawned {
        eprintln!("omega-zero: quiet: no thread: {e}");
    }
}

/// `Some(true)` when the controller is on, `Some(false)` when off, `None` with no switch.
/// The kernel shows `ehci:1, probe:1`; the last number is whether it is running.
fn controller_on(switch: &Path) -> Option<bool> {
    let v = fs::read_to_string(switch).ok()?;
    let v = v.trim();
    let last = v.rsplit(':').next().unwrap_or(v).trim();
    Some(last != "0")
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Mock {
        _dir: tempfile::TempDir,
        quiet: Quiet,
    }

    fn mock() -> Mock {
        let dir = tempfile::tempdir().unwrap();
        let sys = dir.path().join("sys");
        let soc = sys.join("devices/platform/soc");
        for (dev, file) in IDLE_CONTROLLERS {
            fs::create_dir_all(soc.join(dev)).unwrap();
            let n = &dev[dev.len() - 12..dev.len() - 11];
            let kind = &file[..4];
            fs::write(soc.join(dev).join(file), format!("{kind}:{n}, probe:1\n")).unwrap();
        }
        // The USB-C port's own controllers, which must never be switched off.
        for (dev, file) in [
            ("5100000.udc-controller", "udc_enable"),
            ("5101000.ehci0-controller", "ehci_enable"),
            ("5101000.ohci0-controller", "ohci_enable"),
        ] {
            fs::create_dir_all(soc.join(dev)).unwrap();
            fs::write(soc.join(dev).join(file), "x:0, probe:1\n").unwrap();
        }
        Mock {
            _dir: dir,
            quiet: Quiet { sys },
        }
    }

    #[test]
    fn all_six_controllers_switch_off() {
        let m = mock();
        let q = &m.quiet;
        assert_eq!(q.usb_off(), 6);
        for (dev, file) in IDLE_CONTROLLERS {
            let v = fs::read_to_string(q.sys.join("devices/platform/soc").join(dev).join(file))
                .unwrap();
            assert_eq!(v.trim(), "0", "{dev}");
        }
    }

    #[test]
    fn the_usb_c_port_is_never_touched() {
        let m = mock();
        let q = &m.quiet;
        assert_eq!(q.run(), QuietReport { usb: 6 });
        for (dev, file) in [
            ("5100000.udc-controller", "udc_enable"),
            ("5101000.ehci0-controller", "ehci_enable"),
            ("5101000.ohci0-controller", "ohci_enable"),
        ] {
            let v = fs::read_to_string(q.sys.join("devices/platform/soc").join(dev).join(file))
                .unwrap();
            assert_eq!(v, "x:0, probe:1\n", "{dev} was written");
        }
    }

    #[test]
    fn a_missing_controller_is_skipped_and_the_rest_still_go_off() {
        let m = mock();
        let q = &m.quiet;
        fs::remove_dir_all(q.sys.join("devices/platform/soc/5310000.ehci2-controller")).unwrap();
        assert_eq!(q.usb_off(), 5);
    }

    #[test]
    fn a_controller_already_off_counts_as_off() {
        let m = mock();
        let q = &m.quiet;
        let switch = q
            .sys
            .join("devices/platform/soc/5200000.ohci1-controller/ohci_enable");
        fs::write(&switch, "ohci:1, probe:0\n").unwrap();
        assert_eq!(q.usb_off(), 6);
        assert_eq!(
            fs::read_to_string(&switch).unwrap(),
            "ohci:1, probe:0\n",
            "an idle controller was written again"
        );
    }
}
