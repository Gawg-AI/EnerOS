//! mini XML DOM 解析器（D6：crate 内私有，零 unsafe、零第三方依赖）.
//!
//! 仅覆盖 SCL 子集所需能力：元素、属性、文本、嵌套、自闭合标签、
//! XML 声明跳过、注释跳过、CDATA 文本、5 个预定义实体转义
//! （`&amp;` `&lt;` `&gt;` `&quot;` `&apos;）。
//! 所有解析错误携带 `line:column` 位置（`ModelError::SclParseError`）。
//!
//! 实现说明：按字节扫描 ASCII 结构字符（`<` `>` `"` 等不可能出现在
//! UTF-8 多字节序列中），文本按 UTF-8 切片，因此对非 ASCII 内容安全。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::ModelError;

/// XML 元素节点（DOM）。
pub struct XmlNode {
    /// 标签名。
    pub name: String,
    /// 属性列表（保序）。
    pub attrs: Vec<(String, String)>,
    /// 子元素列表（保序）。
    pub children: Vec<XmlNode>,
    /// 直接文本内容（多段以空格连接，实体已解码）。
    pub text: String,
}

impl XmlNode {
    /// 按名取属性值。
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

/// 收集所有匹配 tag 的后代元素（先根序）。
pub fn collect_descendants<'a>(node: &'a XmlNode, tag: &str, out: &mut Vec<&'a XmlNode>) {
    for child in &node.children {
        if child.name == tag {
            out.push(child);
        }
        collect_descendants(child, tag, out);
    }
}

/// 查找首个匹配 tag 的后代元素（先根序）。
pub fn find_descendant<'a>(node: &'a XmlNode, tag: &str) -> Option<&'a XmlNode> {
    for child in &node.children {
        if child.name == tag {
            return Some(child);
        }
        if let Some(found) = find_descendant(child, tag) {
            return Some(found);
        }
    }
    None
}

/// 解析完整 XML 文档（跳过声明/注释，要求单一根元素）。
pub fn parse_document(s: &str) -> Result<XmlNode, ModelError> {
    let mut p = Parser::new(s);
    p.skip_misc()?;
    if p.peek() != Some(b'<') {
        return Err(p.err("expected root element"));
    }
    let root = p.parse_element()?;
    p.skip_misc()?;
    if p.pos < p.s.len() {
        return Err(p.err("trailing content after root element"));
    }
    Ok(root)
}

/// 递归下降解析器（逐字节扫描，跟踪 line/column）。
struct Parser<'a> {
    s: &'a str,
    pos: usize,
    line: usize,
    col: usize,
}

impl<'a> Parser<'a> {
    fn new(s: &'a str) -> Self {
        Self {
            s,
            pos: 0,
            line: 1,
            col: 1,
        }
    }

    /// 构造携带 line:column 位置的解析错误。
    fn err(&self, msg: &str) -> ModelError {
        ModelError::SclParseError(format!("{} at line {}:{}", msg, self.line, self.col))
    }

    fn peek(&self) -> Option<u8> {
        self.s.as_bytes().get(self.pos).copied()
    }

    fn bump(&mut self) {
        if let Some(b) = self.peek() {
            self.pos += 1;
            if b == b'\n' {
                self.line += 1;
                self.col = 1;
            } else {
                self.col += 1;
            }
        }
    }

    /// 字节级前缀匹配（对任意字节位置安全，无 UTF-8 切片风险）。
    fn starts_with(&self, pat: &str) -> bool {
        self.s.as_bytes()[self.pos..].starts_with(pat.as_bytes())
    }

    fn eat_str(&mut self, pat: &str) -> bool {
        if self.starts_with(pat) {
            for _ in 0..pat.len() {
                self.bump();
            }
            true
        } else {
            false
        }
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.bump();
        }
    }

    /// 跳过连续的 XML 声明（`<?...?>`）与注释（`<!--...-->`）。
    fn skip_misc(&mut self) -> Result<(), ModelError> {
        loop {
            self.skip_ws();
            if self.starts_with("<?") {
                self.skip_pi()?;
            } else if self.starts_with("<!--") {
                self.skip_comment()?;
            } else {
                return Ok(());
            }
        }
    }

    fn skip_pi(&mut self) -> Result<(), ModelError> {
        self.bump();
        self.bump();
        while self.pos < self.s.len() && !self.starts_with("?>") {
            self.bump();
        }
        if !self.eat_str("?>") {
            return Err(self.err("unterminated processing instruction"));
        }
        Ok(())
    }

    fn skip_comment(&mut self) -> Result<(), ModelError> {
        for _ in 0..4 {
            self.bump();
        }
        while self.pos < self.s.len() && !self.starts_with("-->") {
            self.bump();
        }
        if !self.eat_str("-->") {
            return Err(self.err("unterminated comment"));
        }
        Ok(())
    }

    /// 解析元素：`<name attr="v">...</name>` 或 `<name/>`。
    fn parse_element(&mut self) -> Result<XmlNode, ModelError> {
        self.bump(); // consume '<'
        let name = self.parse_name()?;
        let mut node = XmlNode {
            name,
            attrs: Vec::new(),
            children: Vec::new(),
            text: String::new(),
        };
        loop {
            self.skip_ws();
            match self.peek() {
                Some(b'/') => {
                    self.bump();
                    if self.peek() != Some(b'>') {
                        return Err(self.err("expected '>' after '/'"));
                    }
                    self.bump();
                    return Ok(node);
                }
                Some(b'>') => {
                    self.bump();
                    break;
                }
                Some(_) => {
                    let (attr_name, attr_val) = self.parse_attribute()?;
                    node.attrs.push((attr_name, attr_val));
                }
                None => return Err(self.err("unexpected EOF in start tag")),
            }
        }
        self.parse_content(&mut node)?;
        Ok(node)
    }

    fn parse_attribute(&mut self) -> Result<(String, String), ModelError> {
        let name = self.parse_name()?;
        self.skip_ws();
        if self.peek() != Some(b'=') {
            return Err(self.err("expected '=' after attribute name"));
        }
        self.bump();
        self.skip_ws();
        let quote = match self.peek() {
            Some(q @ (b'"' | b'\'')) => q,
            _ => return Err(self.err("expected quoted attribute value")),
        };
        self.bump();
        let start = self.pos;
        while let Some(b) = self.peek() {
            if b == quote {
                break;
            }
            self.bump();
        }
        if self.peek() != Some(quote) {
            return Err(self.err("unterminated attribute value"));
        }
        let raw = &self.s[start..self.pos];
        self.bump(); // consume closing quote
        let value = self.decode_entities(raw)?;
        Ok((name, value))
    }

    /// 解析元素内容：文本 + 子元素，直到匹配的闭合标签。
    fn parse_content(&mut self, node: &mut XmlNode) -> Result<(), ModelError> {
        loop {
            let text = self.parse_text()?;
            if !text.is_empty() {
                if !node.text.is_empty() {
                    node.text.push(' ');
                }
                node.text.push_str(&text);
            }
            if self.pos >= self.s.len() {
                return Err(self.err("unexpected EOF: unclosed element"));
            }
            if self.starts_with("</") {
                self.bump();
                self.bump();
                let close = self.parse_name()?;
                self.skip_ws();
                if self.peek() != Some(b'>') {
                    return Err(self.err("expected '>' in close tag"));
                }
                self.bump();
                if close != node.name {
                    return Err(self.err("mismatched close tag"));
                }
                return Ok(());
            } else if self.starts_with("<!--") {
                self.skip_comment()?;
            } else if self.starts_with("<![CDATA[") {
                let cdata = self.parse_cdata()?;
                if !node.text.is_empty() {
                    node.text.push(' ');
                }
                node.text.push_str(&cdata);
            } else if self.starts_with("<?") {
                self.skip_pi()?;
            } else if self.peek() == Some(b'<') {
                let child = self.parse_element()?;
                node.children.push(child);
            } else {
                return Err(self.err("unexpected content"));
            }
        }
    }

    /// 收集到 '<' 为止的文本，裁剪空白并解码实体。
    fn parse_text(&mut self) -> Result<String, ModelError> {
        let start = self.pos;
        while let Some(b) = self.peek() {
            if b == b'<' {
                break;
            }
            self.bump();
        }
        let raw = &self.s[start..self.pos];
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Ok(String::new());
        }
        self.decode_entities(trimmed)
    }

    /// 解析 CDATA 段（原文保留，不解码实体）。
    fn parse_cdata(&mut self) -> Result<String, ModelError> {
        for _ in 0..9 {
            self.bump();
        }
        let start = self.pos;
        while self.pos < self.s.len() && !self.starts_with("]]>") {
            self.bump();
        }
        if self.pos >= self.s.len() {
            return Err(self.err("unterminated CDATA section"));
        }
        let text = String::from(&self.s[start..self.pos]);
        self.eat_str("]]>");
        Ok(text)
    }

    /// 解析 XML 名称（字母/数字/`_`/`-`/`.`/`:`）。
    fn parse_name(&mut self) -> Result<String, ModelError> {
        let start = self.pos;
        while let Some(b) = self.peek() {
            if b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b':') {
                self.bump();
            } else {
                break;
            }
        }
        if self.pos == start {
            return Err(self.err("expected name"));
        }
        Ok(String::from(&self.s[start..self.pos]))
    }

    /// 解码 5 个预定义实体（`&amp;` `&lt;` `&gt;` `&quot;` `&apos;`）。
    fn decode_entities(&self, raw: &str) -> Result<String, ModelError> {
        if !raw.contains('&') {
            return Ok(String::from(raw));
        }
        let mut out = String::with_capacity(raw.len());
        let mut rest = raw;
        while let Some(amp) = rest.find('&') {
            out.push_str(&rest[..amp]);
            let after = &rest[amp + 1..];
            let semi = after
                .find(';')
                .ok_or_else(|| self.err("unterminated entity reference"))?;
            let ch = match &after[..semi] {
                "amp" => '&',
                "lt" => '<',
                "gt" => '>',
                "quot" => '"',
                "apos" => '\'',
                _ => return Err(self.err("unknown entity reference")),
            };
            out.push(ch);
            rest = &after[semi + 1..];
        }
        out.push_str(rest);
        Ok(out)
    }
}
