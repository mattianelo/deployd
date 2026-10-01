use super::*;

// @variants: both
#[test]
fn reports_confirmation_separately_and_never_records_phase_text() -> Result<()> {
    let recorder = Recorder::start();
    let mut recorder = recorder.lock().unwrap();
    recorder.phase("confirmation");
    recorder.phase_started -= std::time::Duration::from_millis(100);
    recorder.started -= std::time::Duration::from_millis(100);
    recorder.phase("private/user/path");
    let report = recorder.report().unwrap();
    assert!(report.confirmation_ms >= 100);
    assert_eq!(report.work_ms, report.elapsed_ms - report.confirmation_ms);
    assert!(!serde_json::to_string(&report)?.contains("private"));
    assert!(recorder.report().is_none());
    Ok(())
}

// @variants: both
#[test]
fn timing_log_is_bounded_and_rejects_links() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("log");
    let report = Recorder::start().lock().unwrap().report().unwrap();
    fs::write(&path, vec![b'x'; 256 * 1024])?;
    write_report(&path, &report)?;
    assert!(fs::metadata(&path)?.len() < 4096);
    fs::remove_file(&path)?;
    let target = temp.path().join("unrelated");
    fs::write(&target, b"preserve")?;
    std::os::unix::fs::symlink(&target, &path)?;
    assert!(write_report(&path, &report).is_err());
    assert_eq!(fs::read(&target)?, b"preserve");
    Ok(())
}
