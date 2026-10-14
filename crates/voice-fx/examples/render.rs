//! Renders a 16-bit mono WAV through every effect, to listen to them:
//! `cargo run --release -p voice-fx --example render -- in.wav out-dir`
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, input, dir] = args.as_slice() else {
        eprintln!("usage: render <in.wav (16-bit mono)> <out-dir>");
        std::process::exit(2);
    };
    let b = std::fs::read(input).expect("read input");
    let rate = u32::from_le_bytes([b[24], b[25], b[26], b[27]]);
    let data = b.windows(4).position(|w| w == b"data").expect("data chunk") + 8;
    let x: Vec<f32> = b[data..].chunks_exact(2).map(|s| i16::from_le_bytes([s[0], s[1]]) as f32 / 32768.0).collect();
    let names = ["none", "anonymous", "deep", "chipmunk", "robot", "hall"];
    for (effect, name) in names.iter().enumerate().skip(1) {
        let mut p = voice_fx::Processor::new(rate as f32, 0x5eed + effect as u32);
        p.set_effect(effect as u32);
        let mut y = x.clone();
        let t = std::time::Instant::now();
        y.chunks_mut(128).for_each(|blk| p.process(blk));
        let secs = x.len() as f64 / rate as f64;
        eprintln!("{name}: {:.1}x real time", secs / t.elapsed().as_secs_f64());
        let mut out = Vec::new();
        let len = (y.len() * 2) as u32;
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + len).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        for v in [16u32.to_le_bytes(), [1, 0, 1, 0], rate.to_le_bytes(), (rate * 2).to_le_bytes(), [2, 0, 16, 0]] {
            out.extend_from_slice(&v);
        }
        out.extend_from_slice(b"data");
        out.extend_from_slice(&len.to_le_bytes());
        for v in y {
            out.extend_from_slice(&((v.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
        }
        std::fs::write(format!("{dir}/{name}.wav"), out).expect("write");
    }
}
