fn main() {
    let args: Vec<String> = std::env::args().collect();
    println!("hello from rust, args={}", args.len());
    std::process::exit(5);
}
