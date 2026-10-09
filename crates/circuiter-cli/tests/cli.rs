use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn circuiter(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_circuiter")).args(args).output().unwrap()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn example(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples").join(name)
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let path = entry.unwrap().path();
        let dest = to.join(path.file_name().unwrap());
        if path.is_dir() {
            copy_dir(&path, &dest);
        } else {
            std::fs::copy(&path, &dest).unwrap();
        }
    }
}

#[test]
fn example_checks_clean_and_elaborates() {
    let dir = example("cmos_inverter");
    let check = circuiter(&["-C", dir.to_str().unwrap(), "check"]);
    assert!(check.status.success(), "{}", stdout(&check));
    assert!(stdout(&check).contains("0 error(s), 0 warning(s)"));

    let tree = circuiter(&["-C", dir.to_str().unwrap(), "tree"]);
    assert!(tree.status.success());
    let text = stdout(&tree);
    assert!(text.contains("U1  cmos_inverter:inverter (schematic)"), "{text}");
    assert!(text.contains("MP  std:pmos (spice)"), "{text}");
    assert!(text.contains("5 leaf instance(s)"), "{text}");
}

#[test]
fn example_files_are_canonical() {
    let tmp = tempfile::tempdir().unwrap();
    copy_dir(&example("cmos_inverter"), tmp.path());
    assert!(circuiter(&["-C", tmp.path().to_str().unwrap(), "fmt"]).status.success());
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        for entry in std::fs::read_dir(tmp.path().join(&rel)).unwrap() {
            let name = rel.join(entry.unwrap().file_name());
            if tmp.path().join(&name).is_dir() {
                stack.push(name);
                continue;
            }
            let formatted = std::fs::read_to_string(tmp.path().join(&name)).unwrap();
            let original = std::fs::read_to_string(example("cmos_inverter").join(&name)).unwrap();
            assert_eq!(
                formatted,
                original,
                "{} is not canonical; run `circuiter fmt`",
                name.display()
            );
        }
    }
}

#[test]
fn new_project_checks_clean() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("my-amp");
    let new = circuiter(&["new", dir.to_str().unwrap()]);
    assert!(new.status.success(), "{}", String::from_utf8_lossy(&new.stderr));
    assert!(stdout(&new).contains("Created project `my_amp`"));
    assert!(circuiter(&["-C", dir.to_str().unwrap(), "check"]).status.success());
}

#[test]
fn check_fails_on_errors() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(circuiter(&["new", tmp.path().to_str().unwrap(), "--name", "broken"]).status.success());
    std::fs::write(
        tmp.path().join("cells/top/schematic.ron"),
        r#"(instances: [(name: "R1", cell: "std:resistor", placement: (position: (0, 0)), params: {"R": "abc"})])"#,
    )
    .unwrap();
    let check = circuiter(&["-C", tmp.path().to_str().unwrap(), "check"]);
    assert!(!check.status.success());
    assert!(stdout(&check).contains("invalid quantity `abc`"), "{}", stdout(&check));
}

#[test]
fn lib_show_works_outside_a_project() {
    let tmp = tempfile::tempdir().unwrap();
    let out = circuiter(&["-C", tmp.path().to_str().unwrap(), "lib", "show", "std:clock"]);
    assert!(out.status.success());
    assert!(stdout(&out).contains("frequency"), "{}", stdout(&out));
}

#[test]
fn nets_reports_connectivity_of_the_example() {
    let dir = example("cmos_inverter");
    let out = circuiter(&["-C", dir.to_str().unwrap(), "nets", "inverter"]);
    assert!(out.status.success());
    let text = stdout(&out);
    assert!(text.contains("out        MN.d MP.d port:out"), "{text}");
    assert!(
        text.contains("4 net(s), 2 junction(s), 0 unconnected pin(s), 0 unconnected wire end(s)"),
        "{text}"
    );
}

#[test]
fn render_writes_an_svg() {
    let tmp = tempfile::tempdir().unwrap();
    let svg = tmp.path().join("tb.svg");
    let dir = example("cmos_inverter");
    let out = circuiter(&[
        "-C",
        dir.to_str().unwrap(),
        "render",
        "-o",
        svg.to_str().unwrap(),
        "--theme",
        "bench",
    ]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = std::fs::read_to_string(svg).unwrap();
    assert!(text.starts_with("<svg") && text.contains(">VCLK</text>"), "{text}");
}
