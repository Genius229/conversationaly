//! Hardware-free check against the installer FFmpeg, not a fake microphone.
#[path = "../src/audio/directshow/pcm.rs"]
mod pcm;

#[test]
#[ignore = "requires explicit installer FFmpeg path; run by the desktop Windows gate"]
fn bundled_ffmpeg_emits_exact_pcm_with_a_short_final_tail() {
    let path = std::env::var_os("DIRECTSHOW_TEST_FFMPEG")
        .expect("DIRECTSHOW_TEST_FFMPEG must name the exact installer binary");
    assert!(std::path::Path::new(&path).is_file());
    #[cfg(target_os = "windows")]
    {
        let devices = std::process::Command::new(&path)
            .args(["-hide_banner", "-devices"])
            .output()
            .expect("list compiled FFmpeg devices");
        assert!(devices.status.success());
        let text = String::from_utf8_lossy(&devices.stdout).to_string()
            + &String::from_utf8_lossy(&devices.stderr);
        assert!(
            text.lines().any(|line| {
                let fields: Vec<_> = line.split_whitespace().collect();
                fields.len() >= 2 && fields[0].contains('D') && fields[1] == "dshow"
            }),
            "installer FFmpeg must contain DirectShow input support"
        );
    }
    let output = std::process::Command::new(path)
        .args([
            "-nostdin",
            "-hide_banner",
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=733:sample_rate=48000:duration=0.137",
            "-ac",
            "1",
            "-ar",
            "48000",
            "-c:a",
            "pcm_f32le",
            "-f",
            "f32le",
            "pipe:1",
        ])
        .output()
        .expect("generate local PCM without audio hardware");
    assert!(output.status.success(), "FFmpeg PCM generation failed");
    assert!(output.stderr.is_empty());
    assert_eq!(output.stdout.len(), 6576 * 4);
    let expected: Vec<_> = output
        .stdout
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect();
    let mut actual = Vec::new();
    let mut sizes = Vec::new();
    let mut decoder = pcm::PcmDecoder::new();
    let mut emit = |samples: &[f32]| {
        assert!(samples.iter().all(|s| s.is_finite()));
        sizes.push(samples.len());
        actual.extend_from_slice(samples);
        Ok(())
    };
    let mut delivered = 0;
    for chunk in output.stdout.chunks(7) {
        delivered += decoder.push(chunk, &mut emit).unwrap();
    }
    delivered += decoder.finish(&mut emit).unwrap();
    assert_eq!(delivered, 6576);
    assert_eq!(actual, expected);
    assert_eq!(sizes.last(), Some(&336));
    assert!(sizes[..sizes.len() - 1].iter().all(|&n| n == 480));
    assert!(actual.iter().any(|s| s.abs() > 0.01));
}
