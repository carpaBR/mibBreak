//! Audio track of the RTSP stream: AAC (RTP "mpeg4-generic", RFC 3640) or G.711.

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioCodec {
    /// AAC with its AudioSpecificConfig (from the SDP `config=` hex).
    Aac { config: Vec<u8>, size_length: u32, index_length: u32 },
    /// G.711 A-law
    Pcma,
    /// G.711 µ-law
    Pcmu,
}

#[derive(Debug, Clone)]
pub struct AudioTrack {
    pub control: String,
    pub payload_type: u8,
    pub codec: AudioCodec,
    pub clock_rate: u32,
    pub channels: u32,
}

#[derive(Debug, Clone)]
pub struct AudioFrame {
    /// One AAC access unit (raw, no ADTS) or a run of G.711 samples.
    pub data: Vec<u8>,
    /// RTP timestamp (clock_rate units).
    pub timestamp: u32,
}

fn hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

pub fn parse_sdp_audio(sdp: &str) -> Option<AudioTrack> {
    let mut in_audio = false;
    let mut control = String::new();
    let mut pt: i32 = -1;
    let mut rtpmap: Option<(String, u32, u32)> = None;
    let mut fmtp = String::new();
    for line in sdp.lines() {
        let line = line.trim_end();
        if let Some(m) = line.strip_prefix("m=") {
            if in_audio {
                break;
            }
            in_audio = m.starts_with("audio");
            if in_audio {
                pt = m.split(' ').nth(3).and_then(|v| v.parse().ok()).unwrap_or(-1);
            }
            continue;
        }
        if !in_audio {
            continue;
        }
        if let Some(c) = line.strip_prefix("a=control:") {
            control = c.trim().to_string();
        }
        if let Some(rest) = line.strip_prefix("a=rtpmap:") {
            let mut it = rest.splitn(2, ' ');
            let n: i32 = it.next().and_then(|v| v.parse().ok()).unwrap_or(-2);
            if n == pt {
                let mut enc = it.next().unwrap_or("").split('/');
                let name = enc.next().unwrap_or("").to_ascii_uppercase();
                let rate = enc.next().and_then(|v| v.parse().ok()).unwrap_or(8000);
                let ch = enc.next().and_then(|v| v.parse().ok()).unwrap_or(1);
                rtpmap = Some((name, rate, ch));
            }
        }
        if let Some(rest) = line.strip_prefix(&format!("a=fmtp:{pt} ")) {
            fmtp = rest.to_string();
        }
    }
    if !in_audio && rtpmap.is_none() {
        return None;
    }
    // Static payload types without rtpmap
    let (name, clock_rate, channels) = rtpmap.unwrap_or_else(|| match pt {
        0 => ("PCMU".into(), 8000, 1),
        8 => ("PCMA".into(), 8000, 1),
        _ => (String::new(), 0, 0),
    });
    let params: HashMap<String, String> = fmtp
        .split(';')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let codec = match name.as_str() {
        "MPEG4-GENERIC" => AudioCodec::Aac {
            config: params.get("config").and_then(|c| hex(c)).unwrap_or_default(),
            size_length: params.get("sizelength").and_then(|v| v.parse().ok()).unwrap_or(13),
            index_length: params.get("indexlength").and_then(|v| v.parse().ok()).unwrap_or(3),
        },
        "PCMA" => AudioCodec::Pcma,
        "PCMU" => AudioCodec::Pcmu,
        _ => return None,
    };
    Some(AudioTrack { control, payload_type: pt as u8, codec, clock_rate, channels })
}

/// RTP audio → frames.
pub fn depacketize(track: &AudioTrack, rtp: &[u8], out: &mut Vec<AudioFrame>) {
    if rtp.len() < 12 || rtp[0] >> 6 != 2 || rtp[1] & 0x7f != track.payload_type {
        return;
    }
    let csrc = (rtp[0] & 0x0f) as usize;
    let mut off = 12 + csrc * 4;
    if rtp[0] & 0x10 != 0 && rtp.len() >= off + 4 {
        off += 4 + u16::from_be_bytes([rtp[off + 2], rtp[off + 3]]) as usize * 4;
    }
    let mut end = rtp.len();
    if rtp[0] & 0x20 != 0 {
        end = end.saturating_sub(rtp[rtp.len() - 1] as usize);
    }
    if off >= end {
        return;
    }
    let ts = u32::from_be_bytes([rtp[4], rtp[5], rtp[6], rtp[7]]);
    let p = &rtp[off..end];
    match &track.codec {
        AudioCodec::Pcma | AudioCodec::Pcmu => out.push(AudioFrame { data: p.to_vec(), timestamp: ts }),
        AudioCodec::Aac { size_length, index_length, .. } => {
            // AU-headers-length (bits), then one header per access unit
            if p.len() < 2 {
                return;
            }
            let header_bits = u16::from_be_bytes([p[0], p[1]]) as usize;
            let per = (*size_length + *index_length) as usize;
            if per == 0 || per > 32 {
                return;
            }
            let header_bytes = header_bits.div_ceil(8);
            if p.len() < 2 + header_bytes {
                return;
            }
            let headers = &p[2..2 + header_bytes];
            let mut data = &p[2 + header_bytes..];
            let read_bits = |bit: usize, n: usize| -> u32 {
                let mut v = 0u32;
                for i in 0..n {
                    let b = bit + i;
                    v = (v << 1) | ((headers[b / 8] >> (7 - b % 8)) & 1) as u32;
                }
                v
            };
            let count = header_bits / per;
            for i in 0..count {
                let size = read_bits(i * per, *size_length as usize) as usize;
                if size > data.len() {
                    return;
                }
                // 1024 samples per AAC frame
                out.push(AudioFrame { data: data[..size].to_vec(), timestamp: ts.wrapping_add(i as u32 * 1024) });
                data = &data[size..];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SDP: &str = "v=0\r\nm=video 0 RTP/AVP 96\r\na=rtpmap:96 H264/90000\r\n\
m=audio 0 RTP/AVP 97\r\na=control:trackID=1\r\na=rtpmap:97 MPEG4-GENERIC/16000\r\n\
a=fmtp:97 streamtype=5;profile-level-id=15;mode=AAC-hbr;config=1408;sizeLength=13;indexLength=3;indexDeltaLength=3\r\n";

    #[test]
    fn sdp_aac() {
        let t = parse_sdp_audio(SDP).unwrap();
        assert_eq!(t.payload_type, 97);
        assert_eq!(t.clock_rate, 16000);
        assert_eq!(t.control, "trackID=1");
        assert_eq!(t.codec, AudioCodec::Aac { config: vec![0x14, 0x08], size_length: 13, index_length: 3 });
    }

    #[test]
    fn aac_access_units() {
        let t = parse_sdp_audio(SDP).unwrap();
        // two AUs of 3 and 2 bytes: headers = (3<<3), (2<<3) → 16 bits each
        let mut rtp = vec![0x80, 97, 0, 1, 0, 0, 0, 100, 0, 0, 0, 0];
        rtp.extend_from_slice(&[0x00, 0x20, 0x00, 0x18, 0x00, 0x10, 1, 2, 3, 4, 5]);
        let mut out = Vec::new();
        depacketize(&t, &rtp, &mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].data, vec![1, 2, 3]);
        assert_eq!(out[1].data, vec![4, 5]);
        assert_eq!(out[1].timestamp, 100 + 1024);
    }

    #[test]
    fn g711_static() {
        let t = parse_sdp_audio("m=audio 0 RTP/AVP 8\r\na=control:trackID=1\r\n").unwrap();
        assert_eq!(t.codec, AudioCodec::Pcma);
        assert_eq!(t.clock_rate, 8000);
    }
}
