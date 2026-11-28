//! Tests for `image/mkosi.extra/usr/lib/carrel/session-env` and
//! `image/mkosi.extra/usr/lib/carrel/keyboard-layout`.
//!
//! Runs the real POSIX shell helpers against temporary settings and
//! `/etc/default/keyboard` files so XKB environment generation and vconsole
//! keymap resolution are verified without booting a VM.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use carrel_shell::locale::KEYBOARDS;
use carrel_shell::settings::{settings_path, Settings};
use tempfile::tempdir;

fn carrel_lib_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .join("image/mkosi.extra/usr/lib/carrel")
}

fn run_session_env(
    run_dir: &Path,
    settings_file: &Path,
    etc_keyboard: &Path,
    detect_virt: &str,
) -> String {
    let script = carrel_lib_dir().join("session-env");
    let output = Command::new(&script)
        .env("CARREL_RUN_DIR", run_dir)
        .env("CARREL_SETTINGS_FILE", settings_file)
        .env("CARREL_ETC_KEYBOARD", etc_keyboard)
        .env("CARREL_DETECT_VIRT", detect_virt)
        .output()
        .expect("execute session-env");
    assert!(
        output.status.success(),
        "session-env failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::read_to_string(run_dir.join("session.env")).expect("read session.env")
}

fn run_keyboard_helper(settings_file: &Path, etc_keyboard: &Path, flag: &str) -> String {
    let script = carrel_lib_dir().join("keyboard-layout");
    let output = Command::new(&script)
        .arg(flag)
        .env("CARREL_SETTINGS_FILE", settings_file)
        .env("CARREL_ETC_KEYBOARD", etc_keyboard)
        .output()
        .expect("execute keyboard-layout");
    assert!(
        output.status.success(),
        "keyboard-layout {flag} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("utf-8 stdout")
}

#[test]
fn writes_saved_swedish_layout_and_software_render_flags() {
    let dir = tempdir().unwrap();
    let run_dir = dir.path().join("run");
    let writing = dir.path().join("writing");
    fs::create_dir_all(writing.join(".carrel")).unwrap();
    let settings = Settings {
        keyboard: "se".to_owned(),
        ..Settings::default()
    };
    settings.save(&writing).unwrap();

    let missing_etc = dir.path().join("no-etc-keyboard");
    let env_gpu = run_session_env(&run_dir, &settings_path(&writing), &missing_etc, "no");
    assert_eq!(env_gpu, "CARREL_SESSION=1\nXKB_DEFAULT_LAYOUT=se\n");
    assert!(run_dir.join("session-start").exists());

    let env_vm = run_session_env(&run_dir, &settings_path(&writing), &missing_etc, "yes");
    assert_eq!(
        env_vm,
        "CARREL_SESSION=1\nWLR_RENDERER=pixman\nGSK_RENDERER=cairo\nXKB_DEFAULT_LAYOUT=se\n"
    );
}

#[test]
fn falls_back_to_etc_default_keyboard_with_variant_when_settings_missing() {
    let dir = tempdir().unwrap();
    let run_dir = dir.path().join("run");
    let missing_settings = dir.path().join("missing-settings.json");
    let etc_keyboard = dir.path().join("keyboard");
    fs::write(
        &etc_keyboard,
        "# image default\nXKBLAYOUT=\"de,us\"\nXKBVARIANT=\"nodeadkeys,\"\n",
    )
    .unwrap();

    let env = run_session_env(&run_dir, &missing_settings, &etc_keyboard, "no");
    assert_eq!(
        env,
        "CARREL_SESSION=1\nXKB_DEFAULT_LAYOUT=de\nXKB_DEFAULT_VARIANT=nodeadkeys\n"
    );
    assert_eq!(
        run_keyboard_helper(&missing_settings, &etc_keyboard, "--vconsole").trim(),
        "de-latin1"
    );
}

#[test]
fn saved_setting_overrides_etc_default_keyboard_without_leaking_variant() {
    let dir = tempdir().unwrap();
    let run_dir = dir.path().join("run");
    let writing = dir.path().join("writing");
    fs::create_dir_all(writing.join(".carrel")).unwrap();
    let settings = Settings {
        keyboard: "se".to_owned(),
        ..Settings::default()
    };
    settings.save(&writing).unwrap();

    let etc_keyboard = dir.path().join("keyboard");
    fs::write(
        &etc_keyboard,
        "XKBLAYOUT=\"de\"\nXKBVARIANT=\"nodeadkeys\"\n",
    )
    .unwrap();

    let env = run_session_env(&run_dir, &settings_path(&writing), &etc_keyboard, "no");
    assert_eq!(env, "CARREL_SESSION=1\nXKB_DEFAULT_LAYOUT=se\n");
}

#[test]
fn supports_layout_variant_in_settings_json() {
    let dir = tempdir().unwrap();
    let run_dir = dir.path().join("run");
    let settings_file = dir.path().join("settings.json");
    let missing_etc = dir.path().join("missing-keyboard");

    fs::write(&settings_file, r#"{"keyboard":"de(nodeadkeys)"}"#).unwrap();
    let env = run_session_env(&run_dir, &settings_file, &missing_etc, "no");
    assert_eq!(
        env,
        "CARREL_SESSION=1\nXKB_DEFAULT_LAYOUT=de\nXKB_DEFAULT_VARIANT=nodeadkeys\n"
    );

    fs::write(
        &settings_file,
        "{\n  \"keyboard\": \"us\",\n  \"keyboard_variant\": \"intl\"\n}\n",
    )
    .unwrap();
    let env = run_session_env(&run_dir, &settings_file, &missing_etc, "no");
    assert_eq!(
        env,
        "CARREL_SESSION=1\nXKB_DEFAULT_LAYOUT=us\nXKB_DEFAULT_VARIANT=intl\n"
    );
}

#[test]
fn rejects_unsafe_or_missing_values_and_falls_back_to_us() {
    let dir = tempdir().unwrap();
    let run_dir = dir.path().join("run");
    let settings_file = dir.path().join("settings.json");
    let missing_etc = dir.path().join("missing-keyboard");

    // Missing files -> us.
    let env = run_session_env(&run_dir, &settings_file, &missing_etc, "no");
    assert_eq!(env, "CARREL_SESSION=1\nXKB_DEFAULT_LAYOUT=us\n");

    // Hand-edited garbage / injection attempt -> rejected, falls back to us.
    fs::write(&settings_file, r#"{"keyboard":"se; rm -rf /"}"#).unwrap();
    let env = run_session_env(&run_dir, &settings_file, &missing_etc, "no");
    assert_eq!(env, "CARREL_SESSION=1\nXKB_DEFAULT_LAYOUT=us\n");
}

#[test]
fn every_carrel_keyboard_layout_maps_to_xkb_and_vconsole() {
    let dir = tempdir().unwrap();
    let settings_file = dir.path().join("settings.json");
    let missing_etc = dir.path().join("missing-keyboard");

    for (code, _label) in KEYBOARDS {
        fs::write(&settings_file, format!(r#"{{"keyboard":"{code}"}}"#)).unwrap();
        let xkb = run_keyboard_helper(&settings_file, &missing_etc, "--xkb");
        assert_eq!(xkb, format!("XKB_DEFAULT_LAYOUT={code}\n"));
        let vconsole = run_keyboard_helper(&settings_file, &missing_etc, "--vconsole");
        let vconsole = vconsole.trim();
        assert!(!vconsole.is_empty(), "layout {code} has no vconsole keymap");
        if *code == "se" {
            assert_eq!(vconsole, "se-latin1");
        }
        if *code == "gb" {
            assert_eq!(vconsole, "uk");
        }
    }
}
