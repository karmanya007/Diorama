#[path = "../winres.rs"]
mod winres;

fn main() {
    winres::embed("Diorama command line");
}
