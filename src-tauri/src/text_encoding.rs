//! 文本编码识别与严格编解码。自动识别为启发式，写入不允许替换无法表示的字符。
use crate::model::*;
use encoding_rs::Encoding;

pub(crate) fn detect(bytes: &[u8], complete: bool) -> &'static str {
    if bytes.starts_with(b"\xef\xbb\xbf") {
        return "UTF-8 BOM";
    }
    if bytes.starts_with(b"\xff\xfe") {
        return "UTF-16 LE";
    }
    if bytes.starts_with(b"\xfe\xff") {
        return "UTF-16 BE";
    }
    // 常见无 BOM 的 UTF-16 导出含 ASCII 列名；只在明显的交错零字节模式下识别。
    let sample = &bytes[..bytes.len().min(4096)];
    if sample.len() >= 8 {
        let pairs = sample.as_chunks::<2>().0;
        let count = pairs.len();
        let (mut even, mut odd) = (0, 0);
        for pair in pairs {
            even += usize::from(pair[0] == 0);
            odd += usize::from(pair[1] == 0);
        }
        if odd * 4 >= count * 3 && even * 8 < count {
            return "UTF-16 LE 无 BOM";
        }
        if even * 4 >= count * 3 && odd * 8 < count {
            return "UTF-16 BE 无 BOM";
        }
    }
    let mut detector = chardetng::EncodingDetector::new();
    detector.feed(bytes, complete);
    let name = detector.guess(None, true).name();
    if name == "GBK" {
        "GB18030"
    } else {
        name
    }
}

pub(crate) fn decode(bytes: &[u8]) -> Result<(String, &'static str)> {
    let name = detect(bytes, true);
    let (label, skip) = match name {
        "UTF-8 BOM" => ("utf-8", 3),
        "UTF-16 LE" => ("utf-16le", 2),
        "UTF-16 BE" => ("utf-16be", 2),
        "UTF-16 LE 无 BOM" => ("utf-16le", 0),
        "UTF-16 BE 无 BOM" => ("utf-16be", 0),
        _ => (name, 0),
    };
    let codec =
        Encoding::for_label(label.as_bytes()).ok_or_else(|| AppError::new("无法识别文本编码"))?;
    let content = codec
        .decode_without_bom_handling_and_without_replacement(&bytes[skip..])
        .ok_or_else(|| {
            AppError::new(format!(
                "文本无法按 {name} 解码，请在只读预览中手动选择编码"
            ))
        })?
        .into_owned();
    if content.contains('\0') {
        return Err(AppError::new("文件包含二进制内容"));
    }
    Ok((content, name))
}

pub(crate) fn encode_legacy(content: &str, name: &str) -> Result<Vec<u8>> {
    let codec = Encoding::for_label(name.as_bytes())
        .ok_or_else(|| AppError::new("未知文本编码，取消保存"))?;
    let (bytes, _, invalid) = codec.encode(content);
    if invalid {
        return Err(AppError::new(format!(
            "修改内容含 {name} 无法表示的字符，已取消保存；原文件未修改"
        )));
    }
    // 部分标准编码器与解码器不完全对称；保存必须能读回相同字符。
    if codec
        .decode_without_bom_handling_and_without_replacement(&bytes)
        .as_deref()
        != Some(content)
    {
        return Err(AppError::new("原编码无法无损保存修改内容，已取消保存"));
    }
    Ok(bytes.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_csv_roundtrip() {
        for (name, text) in [
            ("GBK", "序号,名称\n1,车辆订单"),
            ("Big5", "編號,名稱\n1,車輛訂單"),
            ("Shift_JIS", "番号,名前\n1,日本語の注文"),
            ("EUC-KR", "번호,이름\n1,한국어 주문"),
            ("windows-1252", "id,name\n1,café déjà vu"),
        ] {
            let bytes = encode_legacy(text, name).unwrap();
            let (decoded, detected) = decode(&bytes).unwrap();
            assert_eq!(decoded, text, "{name} detected as {detected}");
            assert_eq!(encode_legacy(&decoded, detected).unwrap(), bytes);
        }
    }
    #[test]
    fn reject_unrepresentable_and_binary() {
        assert!(encode_legacy("😀", "windows-1252").is_err());
        assert!(decode(b"hello\0binary").is_err());
        assert!(decode(b"\xef\xbb\xbf\xff").is_err());
    }
}

#[cfg(test)]
mod roundtrip_tests {
    use super::*;
    #[test]
    fn bom_and_utf16_preserve_encoding() {
        for name in [
            "UTF-8",
            "UTF-8 BOM",
            "UTF-16 LE",
            "UTF-16 BE",
            "UTF-16 LE 无 BOM",
            "UTF-16 BE 无 BOM",
        ] {
            let text = "ID,Name\n1,hello\n2,world";
            let bytes = crate::files::encode(text, name).unwrap();
            let (decoded, detected) = crate::files::decode(&bytes).unwrap();
            assert_eq!(decoded, text);
            assert_eq!(detected, name);
            assert_eq!(crate::files::encode(&decoded, detected).unwrap(), bytes);
        }
        assert!(decode(b"\xff\xfeA").is_err());
    }
    #[test]
    fn gb18030_extended_characters_roundtrip() {
        let text = "序号,名称\n1,车辆订单😀";
        let bytes = encode_legacy(text, "GB18030").unwrap();
        let (decoded, detected) = decode(&bytes).unwrap();
        assert_eq!(decoded, text);
        assert_eq!(encode_legacy(&decoded, detected).unwrap(), bytes);
    }
}
