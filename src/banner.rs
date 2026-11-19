use console::style;

const DISC: [&str; 6] = [
    r#"   .-"""-.   "#,
    r"  /  .-.  \  ",
    r" |  ( o )  | ",
    r"  \  '-'  /  ",
    r"   '-...-'   ",
    r"             ",
];

const TITLE: [&str; 6] = [
    r"          _      _       ",
    r"  ___  __| |_ __(_)_ __  ",
    r" / __|/ _` | '__| | '_ \ ",
    r"| (__| (_| | |  | | |_) |",
    r" \___|\__,_|_|  |_| .__/ ",
    r"                  |_|    ",
];

/// Print the startup banner.
pub fn print() {
    println!();
    for (disc, title) in DISC.iter().zip(TITLE) {
        println!(
            "  {}  {}",
            style(disc).magenta().bold(),
            style(title).cyan().bold()
        );
    }
    println!(
        "  {}",
        style(format!(
            "v{} · rip CDs straight into your music library",
            env!("CARGO_PKG_VERSION")
        ))
        .dim()
    );
    println!();
}
