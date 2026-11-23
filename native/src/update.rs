//! 自动更新里与平台无关的部分：比版本号、读 GitHub 的最新版本信息、读 SHA256SUMS、算 SHA256。
//! 只看输入，方便测试。

use serde_json::Value;

/// 最新正式版的信息（GitHub 的 releases/latest 不含预览版）。
pub const LATEST_URL: &str = "https://api.github.com/repos/MoonFlower-King/island/releases/latest";

/// "v2.0.5"、"2.0.5"、"2.1.0-beta.1" → (2, 0, 5, 预览标记)。
pub fn parse_version(s: &str) -> Option<(u64, u64, u64, Option<String>)> {
    let s = s.trim().trim_start_matches(['v', 'V']);
    let (core, pre) = match s.split_once('-') {
        Some((c, p)) => (c, Some(p.to_string())),
        None => (s, None),
    };
    let mut it = core.split('.').map(|p| p.parse::<u64>().ok());
    let v = (it.next()??, it.next()??, it.next()??);
    if it.next().is_some() {
        return None;
    }
    Some((v.0, v.1, v.2, pre))
}

/// `candidate` 比 `current` 新吗？同一个版本号时，正式版比预览版新。
pub fn is_newer(candidate: &str, current: &str) -> bool {
    let (Some(a), Some(b)) = (parse_version(candidate), parse_version(current)) else {
        return false;
    };
    match (a.0, a.1, a.2).cmp(&(b.0, b.1, b.2)) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => a.3.is_none() && b.3.is_some(),
    }
}

/// 发布包里给这台电脑用的 exe 的文件名。
pub fn exe_asset(version: &str, arm64: bool) -> String {
    let v = version.trim().trim_start_matches(['v', 'V']);
    format!(
        "Island-native-{v}-win-{}.exe",
        if arm64 { "arm64" } else { "x64" }
    )
}

/// 一个可以更新到的版本。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// 不带 v 的版本号。
    pub version: String,
    pub exe_name: String,
    pub exe_url: String,
    pub exe_size: u64,
    pub sums_url: String,
}

/// 从 releases/latest 的 JSON 里找出这台电脑要下载的 exe 和校验文件。缺了任何一个都不更新。
pub fn parse_release(json: &str, arm64: bool) -> Option<Release> {
    let v: Value = serde_json::from_str(json).ok()?;
    if v.get("draft").and_then(Value::as_bool) == Some(true)
        || v.get("prerelease").and_then(Value::as_bool) == Some(true)
    {
        return None;
    }
    let tag = v.get("tag_name")?.as_str()?;
    parse_version(tag)?;
    let version = tag.trim_start_matches(['v', 'V']).to_string();
    let exe_name = exe_asset(&version, arm64);
    let assets = v.get("assets")?.as_array()?;
    let find = |name: &str| {
        assets
            .iter()
            .find(|a| a.get("name").and_then(Value::as_str) == Some(name))
    };
    let exe = find(&exe_name)?;
    let sums = find("SHA256SUMS.txt")?;
    Some(Release {
        version,
        exe_url: exe.get("browser_download_url")?.as_str()?.to_string(),
        exe_size: exe.get("size").and_then(Value::as_u64).unwrap_or(0),
        sums_url: sums.get("browser_download_url")?.as_str()?.to_string(),
        exe_name,
    })
}

/// SHA256SUMS.txt（每行 "哈希  文件名"）里某个文件的哈希（小写十六进制）。
pub fn checksum_for(sums: &str, name: &str) -> Option<String> {
    sums.trim_start_matches('\u{feff}')
        .lines()
        .find_map(|line| {
            let mut parts = line.split_whitespace();
            let hash = parts.next()?;
            let file = parts.next()?.trim_start_matches('*');
            (file == name && hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()))
                .then(|| hash.to_ascii_lowercase())
        })
}

/// SHA256，返回小写十六进制。
pub fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut msg = data.to_vec();
    let bits = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend(bits.to_be_bytes());
    for block in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, word) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let mut v = h;
        for i in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7]
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v = [
                t1.wrapping_add(t2),
                v[0],
                v[1],
                v[2],
                v[3].wrapping_add(t1),
                v[4],
                v[5],
                v[6],
            ];
        }
        for (a, b) in h.iter_mut().zip(v) {
            *a = a.wrapping_add(b);
        }
    }
    h.iter().map(|x| format!("{x:08x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert_eq!(parse_version("v2.0.5"), Some((2, 0, 5, None)));
        assert_eq!(
            parse_version("2.1.0-beta.1"),
            Some((2, 1, 0, Some("beta.1".into())))
        );
        assert_eq!(parse_version("2.0"), None);
        assert_eq!(parse_version("2.0.x"), None);
        assert!(is_newer("v2.0.6", "2.0.5"));
        assert!(is_newer("2.1.0", "2.0.9"));
        assert!(is_newer("3.0.0", "2.10.10"));
        assert!(!is_newer("2.0.5", "2.0.5"));
        assert!(!is_newer("2.0.4", "2.0.5"));
        assert!(is_newer("2.1.0", "2.1.0-beta.1"));
        assert!(!is_newer("2.1.0-beta.1", "2.1.0"));
        assert!(!is_newer("garbage", "2.0.5"));
    }

    #[test]
    fn picks_the_right_assets() {
        let json = r#"{"tag_name":"v2.0.6","draft":false,"prerelease":false,"assets":[
            {"name":"Island-native-2.0.6-win-x64.zip","browser_download_url":"https://x/zip","size":1},
            {"name":"Island-native-2.0.6-win-x64.exe","browser_download_url":"https://x/x64.exe","size":930000},
            {"name":"Island-native-2.0.6-win-arm64.exe","browser_download_url":"https://x/arm.exe","size":850000},
            {"name":"SHA256SUMS.txt","browser_download_url":"https://x/sums","size":300}]}"#;
        let r = parse_release(json, false).unwrap();
        assert_eq!(r.version, "2.0.6");
        assert_eq!(r.exe_name, "Island-native-2.0.6-win-x64.exe");
        assert_eq!(
            (r.exe_url.as_str(), r.exe_size),
            ("https://x/x64.exe", 930000)
        );
        assert_eq!(r.sums_url, "https://x/sums");
        assert_eq!(
            parse_release(json, true).unwrap().exe_url,
            "https://x/arm.exe"
        );
        // 没有 exe（老版本只有 zip）或者是预览版：不更新
        let zip_only = r#"{"tag_name":"v2.0.5","assets":[{"name":"SHA256SUMS.txt","browser_download_url":"s"}]}"#;
        assert!(parse_release(zip_only, false).is_none());
        let pre = json.replace(r#""prerelease":false"#, r#""prerelease":true"#);
        assert!(parse_release(&pre, false).is_none());
    }

    #[test]
    fn reads_checksums() {
        let a = "a".repeat(64);
        let b = "B".repeat(64);
        let sums = format!(
            "\u{feff}{a}  Island-native-2.0.6-win-x64.zip\r\n{b} *Island-native-2.0.6-win-x64.exe\r\nbad line\n"
        );
        assert_eq!(
            checksum_for(&sums, "Island-native-2.0.6-win-x64.exe"),
            Some("b".repeat(64))
        );
        assert_eq!(
            checksum_for(&sums, "Island-native-2.0.6-win-x64.zip"),
            Some(a)
        );
        assert_eq!(checksum_for(&sums, "other.exe"), None);
    }

    #[test]
    fn sha256_test_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        let million = vec![b'a'; 1_000_000];
        assert_eq!(
            sha256_hex(&million),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }
}
