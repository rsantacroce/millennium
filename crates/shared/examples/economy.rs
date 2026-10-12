//! Prints the economy: prices, recipe margins, quest rewards, income from mobs and hacks, and
//! any buy-low-sell-high loops. For balancing content: `cargo run -p shared --example economy`.

fn main() {
    let content = shared::content::Content::builtin();
    print!("{}", shared::economy::report(&content));
    let loops = shared::economy::arbitrage(&content);
    if loops.is_empty() {
        println!("\nNo money loops.");
    } else {
        println!("\n== MONEY LOOPS ==");
        for l in loops {
            println!("{l}");
        }
        std::process::exit(1);
    }
}
