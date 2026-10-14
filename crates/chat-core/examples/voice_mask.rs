//! Masks the voice in a WAV file (see src/voice_mask.rs), to listen to the result:
//! `cargo run --release -p chat-core --example voice_mask -- in.wav out.wav`
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, input, output] = args.as_slice() else {
        eprintln!("usage: voice_mask <in.wav> <out.wav>");
        std::process::exit(2);
    };
    let wav = std::fs::read(input).expect("read input");
    let started = std::time::Instant::now();
    let out = chat_core::voice_mask::mask_wav(&wav).expect("mask");
    std::fs::write(output, out).expect("write output");
    eprintln!("done in {:?}", started.elapsed());
}
