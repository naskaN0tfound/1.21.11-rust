//! Remapping an obfuscated release: Minecraft up to 1.21.11 ships classes named `a`, `bzx$b`, and
//! Mojang publishes the real names beside each version as ProGuard mapping files (the version
//! JSON's `client_mappings` and `server_mappings`). 26.1 and later ship unobfuscated, and
//! everything downstream of the preparer (the Java exporters, the data generator's callers, the
//! decompiler, the harness oracles) is written against the real names. [`remap_jar`] turns an
//! obfuscated jar into one with the real names, so those stay as they are.
//!
//! The rewrite works on the class file's constant pool: every reference keeps its index, and only
//! the strings they point to change, so the bytecode is copied untouched. Class, field and method
//! names, descriptors and generic signatures are rewritten wherever they occur (references,
//! declarations, `Signature`, annotations, `InnerClasses`, `EnclosingMethod`, `Record`, local
//! variable tables, `invokedynamic` names and record `ObjectMethods` bootstraps). A member
//! reference names the class it is used through, which may not be the one that declares it, so
//! members are looked up through the superclasses and interfaces of the jar, as the JVM resolves
//! them. Type annotations (`Runtime*TypeAnnotations`) are copied as they are.
//!
//! The mapping files are Mojang's, for development and for the player's own preparation; neither
//! they nor a remapped jar are ever committed or shipped (`tools/check_public_tree.py`).

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

type Res<T> = Result<T, String>;

/// Bumped when the remapper's output changes: remapped jars are made again.
pub const REMAP_VERSION: u32 = 1;

// ---------------------------------------------------------------------------------- mappings

/// What a ProGuard mapping says about one class. Members are keyed by their obfuscated name and
/// descriptor (both in obfuscated terms, as a class file states them).
#[derive(Default, Debug)]
struct ClassMap {
    /// The real internal name (`net/minecraft/core/BlockPos`).
    name: String,
    /// The class's `SourceFile`, which the mapping states in a comment after the class line.
    source_file: Option<String>,
    fields: HashMap<(String, String), String>,
    methods: HashMap<(String, String), String>,
}

/// A parsed ProGuard mapping: obfuscated internal name to what it maps to.
#[derive(Default, Debug)]
pub struct Mappings {
    classes: HashMap<String, ClassMap>,
}

fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// A source type of the mapping (`java.lang.String[]`, `int`) as a descriptor, with class names
/// taken to their obfuscated names through `to_obf` (real internal name to obfuscated one).
fn source_type_to_desc(ty: &str, to_obf: &HashMap<String, String>) -> String {
    let mut base = ty;
    let mut dims = 0;
    while let Some(rest) = base.strip_suffix("[]") {
        base = rest;
        dims += 1;
    }
    let mut out = "[".repeat(dims);
    out.push_str(match base {
        "byte" => "B",
        "char" => "C",
        "double" => "D",
        "float" => "F",
        "int" => "I",
        "long" => "J",
        "short" => "S",
        "boolean" => "Z",
        "void" => "V",
        _ => {
            let internal = base.replace('.', "/");
            out.push('L');
            out.push_str(to_obf.get(&internal).unwrap_or(&internal));
            out.push(';');
            return out;
        }
    });
    out
}

impl Mappings {
    /// Parses a ProGuard mapping file (Mojang's `client.txt` or `server.txt`). The client and the
    /// server are obfuscated separately, so a mapping goes with the jar of its own side only: the
    /// same obfuscated name in the two files is two different classes.
    pub fn parse(text: &str) -> Res<Mappings> {
        let mut mappings = Mappings::default();
        mappings.add(text)?;
        Ok(mappings)
    }

    /// Adds another mapping file of the same jar (a file is one side's whole mapping, so this is
    /// for tests and tools).
    pub fn add(&mut self, text: &str) -> Res<()> {
        // Real internal name to obfuscated one, over this file's classes.
        let mut to_obf: HashMap<String, String> = HashMap::new();
        for line in text.lines() {
            if line.starts_with('#') || line.starts_with(' ') || line.is_empty() {
                continue;
            }
            let (real, obf) = line
                .strip_suffix(':')
                .and_then(|l| l.split_once(" -> "))
                .ok_or_else(|| format!("Not a class line of a mapping: {line}"))?;
            to_obf.insert(real.replace('.', "/"), obf.replace('.', "/"));
        }
        let mut current: Option<String> = None;
        for line in text.lines() {
            if let Some(json) = line.strip_prefix("# {") {
                // `# {"fileName":"Blaze3D.java","id":"sourceFile"}`
                if json.contains("\"id\":\"sourceFile\"") {
                    let file = json
                        .split_once("\"fileName\":\"")
                        .and_then(|(_, rest)| rest.split_once('"'))
                        .map(|(name, _)| name.to_string());
                    if let (Some(owner), Some(file)) = (&current, file) {
                        if let Some(class) = self.classes.get_mut(owner) {
                            class.source_file = Some(file);
                        }
                    }
                }
                continue;
            }
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            if !line.starts_with(' ') {
                let (real, obf) = line
                    .strip_suffix(':')
                    .and_then(|l| l.split_once(" -> "))
                    .ok_or_else(|| format!("Not a class line of a mapping: {line}"))?;
                let obf = obf.replace('.', "/");
                let entry = self.classes.entry(obf.clone()).or_default();
                entry.name = real.replace('.', "/");
                current = Some(obf);
                continue;
            }
            let Some(owner) = &current else {
                return Err(format!("A member before any class in the mapping: {line}"));
            };
            let (left, obf_name) = line
                .trim_start()
                .split_once(" -> ")
                .ok_or_else(|| format!("Not a member line of a mapping: {line}"))?;
            let mut left = left;
            // `12:14:` line numbers in front of a method.
            while let Some((first, rest)) = left.split_once(':') {
                if is_digits(first) {
                    left = rest;
                } else {
                    break;
                }
            }
            let class = self.classes.get_mut(owner).expect("inserted above");
            if let Some(open) = left.find('(') {
                let close = left[open..]
                    .find(')')
                    .map(|c| c + open)
                    .ok_or_else(|| format!("A method without ')' in the mapping: {line}"))?;
                let (ret, name) = left[..open]
                    .rsplit_once(' ')
                    .ok_or_else(|| format!("A method without a type in the mapping: {line}"))?;
                // A name with a dot is a frame of an inlined method, not a declaration.
                if name.contains('.') || name.starts_with('<') || name == obf_name {
                    continue;
                }
                let args = &left[open + 1..close];
                let mut desc = String::from("(");
                if !args.is_empty() {
                    for arg in args.split(',') {
                        desc.push_str(&source_type_to_desc(arg, &to_obf));
                    }
                }
                desc.push(')');
                desc.push_str(&source_type_to_desc(ret, &to_obf));
                class
                    .methods
                    .insert((obf_name.to_string(), desc), name.to_string());
            } else {
                let (ty, name) = left
                    .rsplit_once(' ')
                    .ok_or_else(|| format!("A field without a type in the mapping: {line}"))?;
                if name == obf_name {
                    continue;
                }
                class.fields.insert(
                    (obf_name.to_string(), source_type_to_desc(ty, &to_obf)),
                    name.to_string(),
                );
            }
        }
        Ok(())
    }

    /// How many classes the mapping names.
    pub fn len(&self) -> usize {
        self.classes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.classes.is_empty()
    }
}

// ---------------------------------------------------------------------------- class file model

/// One constant pool entry. Strings are kept as bytes (modified UTF-8) so that entries nobody
/// remaps are written back exactly.
#[derive(Clone, Debug)]
enum Cp {
    /// Slot 0, and the second slot of a long or double.
    Unusable,
    Utf8(Vec<u8>),
    /// An int (tag 3) or float (tag 4).
    Raw4(u8, [u8; 4]),
    /// A long (tag 5) or double (tag 6).
    Raw8(u8, [u8; 8]),
    Class(u16),
    Str(u16),
    Field(u16, u16),
    Method(u16, u16),
    IMethod(u16, u16),
    NameAndType(u16, u16),
    MethodHandle(u8, u16),
    MethodType(u16),
    Dynamic(u16, u16),
    InvokeDynamic(u16, u16),
    Module(u16),
    Package(u16),
}

#[derive(Clone, Debug)]
struct Attr {
    name: u16,
    data: Vec<u8>,
}

#[derive(Clone, Debug)]
struct Member {
    access: u16,
    name: u16,
    desc: u16,
    attrs: Vec<Attr>,
}

#[derive(Debug)]
struct ClassFile {
    minor: u16,
    major: u16,
    cp: Vec<Cp>,
    access: u16,
    this: u16,
    super_class: u16,
    interfaces: Vec<u16>,
    fields: Vec<Member>,
    methods: Vec<Member>,
    attrs: Vec<Attr>,
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Reader { bytes, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Res<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.bytes.len())
            .ok_or("The class file ends too early")?;
        let out = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(out)
    }
    fn u1(&mut self) -> Res<u8> {
        Ok(self.take(1)?[0])
    }
    fn u2(&mut self) -> Res<u16> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }
    fn u4(&mut self) -> Res<u32> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn done(&self) -> bool {
        self.pos == self.bytes.len()
    }
}

fn put2(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn parse_attrs(r: &mut Reader) -> Res<Vec<Attr>> {
    let count = r.u2()?;
    let mut attrs = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let name = r.u2()?;
        let len = r.u4()? as usize;
        attrs.push(Attr {
            name,
            data: r.take(len)?.to_vec(),
        });
    }
    Ok(attrs)
}

fn write_attrs(out: &mut Vec<u8>, attrs: &[Attr]) {
    put2(out, attrs.len() as u16);
    for a in attrs {
        put2(out, a.name);
        out.extend_from_slice(&(a.data.len() as u32).to_be_bytes());
        out.extend_from_slice(&a.data);
    }
}

fn parse_members(r: &mut Reader) -> Res<Vec<Member>> {
    let count = r.u2()?;
    let mut members = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let access = r.u2()?;
        let name = r.u2()?;
        let desc = r.u2()?;
        let attrs = parse_attrs(r)?;
        members.push(Member {
            access,
            name,
            desc,
            attrs,
        });
    }
    Ok(members)
}

fn write_members(out: &mut Vec<u8>, members: &[Member]) {
    put2(out, members.len() as u16);
    for m in members {
        put2(out, m.access);
        put2(out, m.name);
        put2(out, m.desc);
        write_attrs(out, &m.attrs);
    }
}

impl ClassFile {
    fn parse(bytes: &[u8]) -> Res<ClassFile> {
        let mut r = Reader::new(bytes);
        if r.u4()? != 0xCAFE_BABE {
            return Err("Not a class file".into());
        }
        let minor = r.u2()?;
        let major = r.u2()?;
        let count = r.u2()? as usize;
        let mut cp = Vec::with_capacity(count);
        cp.push(Cp::Unusable);
        while cp.len() < count {
            let tag = r.u1()?;
            let entry = match tag {
                1 => {
                    let len = r.u2()? as usize;
                    Cp::Utf8(r.take(len)?.to_vec())
                }
                3 | 4 => Cp::Raw4(tag, r.take(4)?.try_into().unwrap()),
                5 | 6 => Cp::Raw8(tag, r.take(8)?.try_into().unwrap()),
                7 => Cp::Class(r.u2()?),
                8 => Cp::Str(r.u2()?),
                9 => Cp::Field(r.u2()?, r.u2()?),
                10 => Cp::Method(r.u2()?, r.u2()?),
                11 => Cp::IMethod(r.u2()?, r.u2()?),
                12 => Cp::NameAndType(r.u2()?, r.u2()?),
                15 => Cp::MethodHandle(r.u1()?, r.u2()?),
                16 => Cp::MethodType(r.u2()?),
                17 => Cp::Dynamic(r.u2()?, r.u2()?),
                18 => Cp::InvokeDynamic(r.u2()?, r.u2()?),
                19 => Cp::Module(r.u2()?),
                20 => Cp::Package(r.u2()?),
                other => return Err(format!("Unknown constant pool tag {other}")),
            };
            let wide = matches!(entry, Cp::Raw8(..));
            cp.push(entry);
            if wide {
                cp.push(Cp::Unusable);
            }
        }
        let access = r.u2()?;
        let this = r.u2()?;
        let super_class = r.u2()?;
        let n = r.u2()? as usize;
        let mut interfaces = Vec::with_capacity(n);
        for _ in 0..n {
            interfaces.push(r.u2()?);
        }
        let fields = parse_members(&mut r)?;
        let methods = parse_members(&mut r)?;
        let attrs = parse_attrs(&mut r)?;
        if !r.done() {
            return Err("Bytes after the class file's last attribute".into());
        }
        Ok(ClassFile {
            minor,
            major,
            cp,
            access,
            this,
            super_class,
            interfaces,
            fields,
            methods,
            attrs,
        })
    }

    fn write(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(8192);
        out.extend_from_slice(&0xCAFE_BABEu32.to_be_bytes());
        put2(&mut out, self.minor);
        put2(&mut out, self.major);
        put2(&mut out, self.cp.len() as u16);
        for entry in self.cp.iter().skip(1) {
            match entry {
                Cp::Unusable => {}
                Cp::Utf8(b) => {
                    out.push(1);
                    put2(&mut out, b.len() as u16);
                    out.extend_from_slice(b);
                }
                Cp::Raw4(tag, b) => {
                    out.push(*tag);
                    out.extend_from_slice(b);
                }
                Cp::Raw8(tag, b) => {
                    out.push(*tag);
                    out.extend_from_slice(b);
                }
                Cp::Class(a) => {
                    out.push(7);
                    put2(&mut out, *a);
                }
                Cp::Str(a) => {
                    out.push(8);
                    put2(&mut out, *a);
                }
                Cp::Field(a, b) => {
                    out.push(9);
                    put2(&mut out, *a);
                    put2(&mut out, *b);
                }
                Cp::Method(a, b) => {
                    out.push(10);
                    put2(&mut out, *a);
                    put2(&mut out, *b);
                }
                Cp::IMethod(a, b) => {
                    out.push(11);
                    put2(&mut out, *a);
                    put2(&mut out, *b);
                }
                Cp::NameAndType(a, b) => {
                    out.push(12);
                    put2(&mut out, *a);
                    put2(&mut out, *b);
                }
                Cp::MethodHandle(kind, a) => {
                    out.push(15);
                    out.push(*kind);
                    put2(&mut out, *a);
                }
                Cp::MethodType(a) => {
                    out.push(16);
                    put2(&mut out, *a);
                }
                Cp::Dynamic(a, b) => {
                    out.push(17);
                    put2(&mut out, *a);
                    put2(&mut out, *b);
                }
                Cp::InvokeDynamic(a, b) => {
                    out.push(18);
                    put2(&mut out, *a);
                    put2(&mut out, *b);
                }
                Cp::Module(a) => {
                    out.push(19);
                    put2(&mut out, *a);
                }
                Cp::Package(a) => {
                    out.push(20);
                    put2(&mut out, *a);
                }
            }
        }
        put2(&mut out, self.access);
        put2(&mut out, self.this);
        put2(&mut out, self.super_class);
        put2(&mut out, self.interfaces.len() as u16);
        for i in &self.interfaces {
            put2(&mut out, *i);
        }
        write_members(&mut out, &self.fields);
        write_members(&mut out, &self.methods);
        write_attrs(&mut out, &self.attrs);
        out
    }
}

/// The text of a Utf8 entry, None when it is not one or is not plain UTF-8.
fn utf8(cp: &[Cp], index: u16) -> Option<&str> {
    match cp.get(index as usize)? {
        Cp::Utf8(b) => std::str::from_utf8(b).ok(),
        _ => None,
    }
}

/// The name a Class entry names.
fn class_name(cp: &[Cp], index: u16) -> Option<&str> {
    match cp.get(index as usize)? {
        Cp::Class(n) => utf8(cp, *n),
        _ => None,
    }
}

// ------------------------------------------------------------------------- hierarchy and names

/// A class's superclass and interfaces, in obfuscated names.
#[derive(Default, Clone, Debug)]
struct Supers {
    superclass: Option<String>,
    interfaces: Vec<String>,
}

/// Remaps class files with a [`Mappings`], knowing the classes of the jar being remapped.
pub struct Remapper<'m> {
    maps: &'m Mappings,
    supers: HashMap<String, Supers>,
}

/// Which grammar a generic signature follows.
#[derive(Clone, Copy, PartialEq)]
enum SigKind {
    Class,
    Method,
    Field,
}

/// What the remapper reports about a jar.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    pub classes: usize,
    pub renamed_classes: usize,
    pub resources: usize,
}

impl<'m> Remapper<'m> {
    /// A remapper for classes that extend and implement each other as `classes` (the class files
    /// of the jar) say.
    pub fn new(maps: &'m Mappings, classes: &[&[u8]]) -> Res<Remapper<'m>> {
        let mut supers = HashMap::new();
        for bytes in classes {
            let cf = ClassFile::parse(bytes)?;
            let name = class_name(&cf.cp, cf.this)
                .ok_or("A class without a name")?
                .to_string();
            let superclass = match cf.super_class {
                0 => None,
                i => class_name(&cf.cp, i).map(String::from),
            };
            let interfaces = cf
                .interfaces
                .iter()
                .filter_map(|i| class_name(&cf.cp, *i).map(String::from))
                .collect();
            supers.insert(
                name,
                Supers {
                    superclass,
                    interfaces,
                },
            );
        }
        Ok(Remapper { maps, supers })
    }

    /// The real name of a class, or the name itself when the mapping has none.
    fn map_class<'a>(&'a self, obf: &'a str) -> &'a str {
        match self.maps.classes.get(obf) {
            Some(c) if !c.name.is_empty() => &c.name,
            _ => obf,
        }
    }

    /// A field or method as declared in `owner` or, failing that, in one of its supertypes: the
    /// real name of the first one the mapping renames. `desc` None matches by name alone.
    fn find_member(
        &self,
        owner: &str,
        name: &str,
        desc: Option<&str>,
        field: bool,
    ) -> Option<&str> {
        let mut seen: HashSet<&str> = HashSet::new();
        let mut stack: Vec<&str> = vec![owner];
        while let Some(class) = stack.pop() {
            if !seen.insert(class) {
                continue;
            }
            if let Some(map) = self.maps.classes.get(class) {
                let table = if field { &map.fields } else { &map.methods };
                let hit = match desc {
                    Some(d) => table.get(&(name.to_string(), d.to_string())),
                    None => table
                        .iter()
                        .find(|((n, _), _)| n == name)
                        .map(|(_, real)| real),
                };
                if let Some(real) = hit {
                    return Some(real);
                }
            }
            if let Some(s) = self.supers.get(class) {
                // Interfaces last on the stack come out first: a class before its interfaces is
                // what resolution does, so push them first.
                for i in s.interfaces.iter().rev() {
                    stack.push(i);
                }
                if let Some(sc) = &s.superclass {
                    stack.push(sc);
                }
            }
        }
        None
    }

    /// A descriptor (`(La;I)[Lb;`) with its class names mapped.
    pub fn map_desc(&self, desc: &str) -> String {
        let mut out = String::with_capacity(desc.len() + 16);
        let b = desc.as_bytes();
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'L' {
                match desc[i..].find(';') {
                    Some(end) => {
                        out.push('L');
                        out.push_str(self.map_class(&desc[i + 1..i + end]));
                        out.push(';');
                        i += end + 1;
                    }
                    None => {
                        out.push_str(&desc[i..]);
                        break;
                    }
                }
            } else {
                out.push(b[i] as char);
                i += 1;
            }
        }
        out
    }

    /// A generic signature with its class names mapped; the signature unchanged when it does not
    /// parse.
    fn map_signature(&self, sig: &str, kind: SigKind) -> String {
        let mut p = SigParser {
            c: sig.as_bytes(),
            p: 0,
            out: String::with_capacity(sig.len() + 16),
            r: self,
        };
        let ok = match kind {
            SigKind::Class => p.class_signature(),
            SigKind::Method => p.method_signature(),
            SigKind::Field => p.ref_type(),
        };
        if ok.is_some() && p.p == p.c.len() {
            p.out
        } else {
            sig.to_string()
        }
    }
}

struct SigParser<'a, 'm> {
    c: &'a [u8],
    p: usize,
    out: String,
    r: &'a Remapper<'m>,
}

impl SigParser<'_, '_> {
    fn peek(&self) -> Option<u8> {
        self.c.get(self.p).copied()
    }
    fn copy_until(&mut self, stop: &[u8]) -> Option<&str> {
        let start = self.p;
        while let Some(b) = self.peek() {
            if stop.contains(&b) {
                break;
            }
            self.p += 1;
        }
        std::str::from_utf8(&self.c[start..self.p]).ok()
    }
    fn class_signature(&mut self) -> Option<()> {
        self.type_params()?;
        self.class_type()?;
        while self.p < self.c.len() {
            self.class_type()?;
        }
        Some(())
    }
    fn method_signature(&mut self) -> Option<()> {
        self.type_params()?;
        if self.peek()? != b'(' {
            return None;
        }
        self.out.push('(');
        self.p += 1;
        while self.peek()? != b')' {
            self.any_type()?;
        }
        self.out.push(')');
        self.p += 1;
        self.any_type()?;
        while self.peek() == Some(b'^') {
            self.out.push('^');
            self.p += 1;
            self.ref_type()?;
        }
        Some(())
    }
    fn type_params(&mut self) -> Option<()> {
        if self.peek() != Some(b'<') {
            return Some(());
        }
        self.out.push('<');
        self.p += 1;
        while self.peek()? != b'>' {
            let ident = self.copy_until(b":")?.to_string();
            self.out.push_str(&ident);
            while self.peek() == Some(b':') {
                self.out.push(':');
                self.p += 1;
                if matches!(self.peek()?, b'L' | b'T' | b'[') {
                    self.ref_type()?;
                }
            }
        }
        self.out.push('>');
        self.p += 1;
        Some(())
    }
    fn any_type(&mut self) -> Option<()> {
        match self.peek()? {
            c @ (b'B' | b'C' | b'D' | b'F' | b'I' | b'J' | b'S' | b'Z' | b'V') => {
                self.out.push(c as char);
                self.p += 1;
                Some(())
            }
            _ => self.ref_type(),
        }
    }
    fn ref_type(&mut self) -> Option<()> {
        match self.peek()? {
            b'L' => self.class_type(),
            b'T' => {
                let var = self.copy_until(b";")?.to_string();
                self.out.push_str(&var);
                self.out.push(';');
                self.p += 1;
                Some(())
            }
            b'[' => {
                self.out.push('[');
                self.p += 1;
                self.any_type()
            }
            _ => None,
        }
    }
    fn class_type(&mut self) -> Option<()> {
        if self.peek()? != b'L' {
            return None;
        }
        self.p += 1;
        let mut obf = self.copy_until(b"<;.")?.to_string();
        let mut real = self.r.map_class(&obf).to_string();
        self.out.push('L');
        self.out.push_str(&real);
        loop {
            match self.peek()? {
                b'<' => self.type_args()?,
                b'.' => {
                    self.p += 1;
                    let inner = self.copy_until(b"<;.")?.to_string();
                    obf = format!("{obf}${inner}");
                    let mapped = self.r.map_class(&obf).to_string();
                    let suffix = match mapped.strip_prefix(&format!("{real}$")) {
                        Some(rest) => rest.to_string(),
                        None => mapped.rsplit('$').next().unwrap_or(&mapped).to_string(),
                    };
                    self.out.push('.');
                    self.out.push_str(&suffix);
                    real = mapped;
                }
                b';' => {
                    self.out.push(';');
                    self.p += 1;
                    return Some(());
                }
                _ => return None,
            }
        }
    }
    fn type_args(&mut self) -> Option<()> {
        self.out.push('<');
        self.p += 1;
        while self.peek()? != b'>' {
            match self.peek()? {
                b'*' => {
                    self.out.push('*');
                    self.p += 1;
                }
                c @ (b'+' | b'-') => {
                    self.out.push(c as char);
                    self.p += 1;
                    self.ref_type()?;
                }
                _ => self.ref_type()?,
            }
        }
        self.out.push('>');
        self.p += 1;
        Some(())
    }
}

// --------------------------------------------------------------------------- the constant pool

/// The constant pool being rewritten: the entries so far, and the ones already made, so that the
/// same string or name-and-type is added once.
struct Pool {
    cp: Vec<Cp>,
    utf: HashMap<Vec<u8>, u16>,
    nat: HashMap<(u16, u16), u16>,
}

impl Pool {
    fn new(cp: Vec<Cp>) -> Pool {
        let mut utf = HashMap::new();
        let mut nat = HashMap::new();
        for (i, entry) in cp.iter().enumerate() {
            match entry {
                Cp::Utf8(b) => {
                    utf.entry(b.clone()).or_insert(i as u16);
                }
                Cp::NameAndType(a, b) => {
                    nat.entry((*a, *b)).or_insert(i as u16);
                }
                _ => {}
            }
        }
        Pool { cp, utf, nat }
    }

    fn push(&mut self, entry: Cp) -> Res<u16> {
        if self.cp.len() >= 0xFFFF {
            return Err("The constant pool outgrew 65535 entries".into());
        }
        self.cp.push(entry);
        Ok((self.cp.len() - 1) as u16)
    }

    fn utf8(&mut self, text: &str) -> Res<u16> {
        if let Some(i) = self.utf.get(text.as_bytes()) {
            return Ok(*i);
        }
        let i = self.push(Cp::Utf8(text.as_bytes().to_vec()))?;
        self.utf.insert(text.as_bytes().to_vec(), i);
        Ok(i)
    }

    fn name_and_type(&mut self, name: &str, desc: &str) -> Res<u16> {
        let key = (self.utf8(name)?, self.utf8(desc)?);
        if let Some(i) = self.nat.get(&key) {
            return Ok(*i);
        }
        let i = self.push(Cp::NameAndType(key.0, key.1))?;
        self.nat.insert(key, i);
        Ok(i)
    }
}

/// The bootstrap methods of a class: each one's method handle and arguments.
fn parse_bootstrap(data: &[u8]) -> Res<Vec<(u16, Vec<u16>)>> {
    let mut r = Reader::new(data);
    let n = r.u2()?;
    let mut out = Vec::new();
    for _ in 0..n {
        let method = r.u2()?;
        let argc = r.u2()?;
        let mut args = Vec::new();
        for _ in 0..argc {
            args.push(r.u2()?);
        }
        out.push((method, args));
    }
    Ok(out)
}

fn write_bootstrap(methods: &[(u16, Vec<u16>)]) -> Vec<u8> {
    let mut out = Vec::new();
    put2(&mut out, methods.len() as u16);
    for (method, args) in methods {
        put2(&mut out, *method);
        put2(&mut out, args.len() as u16);
        for a in args {
            put2(&mut out, *a);
        }
    }
    out
}

/// The class a descriptor ends in, `Lx;` or an array of it: `x`.
fn desc_class(desc: &str) -> Option<&str> {
    let d = desc.trim_start_matches('[');
    d.strip_prefix('L')?.strip_suffix(';')
}

/// An inner class's simple name as ASM's remapper derives it: what follows the last `$`, without
/// the digits of a local class's number. None when the name has no `$`.
fn inner_simple_name(full: &str) -> Option<&str> {
    let tail = &full[full.rfind('$')? + 1..];
    Some(tail.trim_start_matches(|c: char| c.is_ascii_digit()))
}

impl Remapper<'_> {
    /// Remaps one class file: the real name of the class, and its bytes.
    pub fn remap_class(&self, bytes: &[u8]) -> Res<(String, Vec<u8>)> {
        let mut cf = ClassFile::parse(bytes)?;
        let orig = cf.cp.clone();
        let this_obf = class_name(&orig, cf.this)
            .ok_or("A class without a name")?
            .to_string();
        let mut pool = Pool::new(std::mem::take(&mut cf.cp));

        // Class entries (an array class's name is a descriptor).
        for (i, entry) in orig.iter().enumerate() {
            if let Cp::Class(n) = entry {
                let Some(name) = utf8(&orig, *n) else {
                    continue;
                };
                let new = if name.starts_with('[') {
                    self.map_desc(name)
                } else {
                    self.map_class(name).to_string()
                };
                if new != name {
                    pool.cp[i] = Cp::Class(pool.utf8(&new)?);
                }
            }
        }

        // Member references: each gets a name and type of its own, since one name and type is
        // shared by references through classes that name it differently.
        for (i, entry) in orig.iter().enumerate() {
            let (class, nat, field) = match entry {
                Cp::Field(c, n) => (*c, *n, true),
                Cp::Method(c, n) | Cp::IMethod(c, n) => (*c, *n, false),
                _ => continue,
            };
            let (Some(owner), Cp::NameAndType(n, d)) =
                (class_name(&orig, class), &orig[nat as usize])
            else {
                continue;
            };
            let (Some(name), Some(desc)) = (utf8(&orig, *n), utf8(&orig, *d)) else {
                continue;
            };
            let new_desc = self.map_desc(desc);
            let new_name = if owner.starts_with('[') {
                name
            } else {
                self.find_member(owner, name, Some(desc), field)
                    .unwrap_or(name)
            };
            if new_name == name && new_desc == desc {
                continue;
            }
            let nat = pool.name_and_type(new_name, &new_desc)?;
            pool.cp[i] = match entry {
                Cp::Field(c, _) => Cp::Field(*c, nat),
                Cp::Method(c, _) => Cp::Method(*c, nat),
                Cp::IMethod(c, _) => Cp::IMethod(*c, nat),
                _ => unreachable!(),
            };
        }

        // Method types.
        for (i, entry) in orig.iter().enumerate() {
            if let Cp::MethodType(d) = entry {
                if let Some(desc) = utf8(&orig, *d) {
                    let new = self.map_desc(desc);
                    if new != desc {
                        pool.cp[i] = Cp::MethodType(pool.utf8(&new)?);
                    }
                }
            }
        }

        // Bootstrap methods, and the call sites that use them.
        let mut bootstrap = None;
        for (k, a) in cf.attrs.iter().enumerate() {
            if utf8(&orig, a.name) == Some("BootstrapMethods") {
                bootstrap = Some((k, parse_bootstrap(&a.data)?));
            }
        }
        let mut bootstrap_changed = false;
        for (i, entry) in orig.iter().enumerate() {
            let (kind_dynamic, bsm, n) = match entry {
                Cp::InvokeDynamic(b, n) => (false, *b, *n),
                Cp::Dynamic(b, n) => (true, *b, *n),
                _ => continue,
            };
            let Cp::NameAndType(nn, dd) = &orig[n as usize] else {
                continue;
            };
            let (Some(name), Some(desc)) = (utf8(&orig, *nn), utf8(&orig, *dd)) else {
                continue;
            };
            let new_desc = self.map_desc(desc);
            let mut new_name = name.to_string();
            if let Some((_, methods)) = &mut bootstrap {
                if let Some((handle, args)) = methods.get_mut(bsm as usize) {
                    let target = match orig.get(*handle as usize) {
                        Some(Cp::MethodHandle(_, r)) => match orig.get(*r as usize) {
                            Some(Cp::Method(c, _) | Cp::IMethod(c, _)) => class_name(&orig, *c),
                            _ => None,
                        },
                        _ => None,
                    };
                    match target {
                        // A lambda or method reference: the call site is named for the method of
                        // the interface it implements, which the call site returns.
                        Some("java/lang/invoke/LambdaMetafactory") if !kind_dynamic => {
                            let sam_desc = match args.first().and_then(|a| orig.get(*a as usize)) {
                                Some(Cp::MethodType(d)) => utf8(&orig, *d),
                                _ => None,
                            };
                            let iface = desc.rsplit(')').next().and_then(desc_class);
                            if let (Some(iface), Some(sam)) = (iface, sam_desc) {
                                if let Some(real) = self.find_member(iface, name, Some(sam), false)
                                {
                                    new_name = real.to_string();
                                }
                            }
                        }
                        // A record's equals, hashCode and toString: the component names are a
                        // `;`-separated string, naming fields of the record.
                        Some("java/lang/runtime/ObjectMethods") if !kind_dynamic => {
                            if let (Some(Cp::Class(_)), Some(Cp::Str(s))) = (
                                args.first().and_then(|a| orig.get(*a as usize)),
                                args.get(1).and_then(|a| orig.get(*a as usize)),
                            ) {
                                let record = args.first().and_then(|a| class_name(&orig, *a));
                                if let (Some(record), Some(names)) = (record, utf8(&orig, *s)) {
                                    let mapped: Vec<&str> = names
                                        .split(';')
                                        .map(|n| {
                                            self.find_member(record, n, None, true).unwrap_or(n)
                                        })
                                        .collect();
                                    let joined = mapped.join(";");
                                    if joined != names {
                                        let u = pool.utf8(&joined)?;
                                        let s = pool.push(Cp::Str(u))?;
                                        args[1] = s;
                                        bootstrap_changed = true;
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            if new_name == name && new_desc == desc {
                continue;
            }
            let nat = pool.name_and_type(&new_name, &new_desc)?;
            pool.cp[i] = if kind_dynamic {
                Cp::Dynamic(bsm, nat)
            } else {
                Cp::InvokeDynamic(bsm, nat)
            };
        }
        if let (true, Some((k, methods))) = (bootstrap_changed, &bootstrap) {
            cf.attrs[*k].data = write_bootstrap(methods);
        }

        // Declarations.
        let own = self.maps.classes.get(&this_obf);
        for field in &mut cf.fields {
            let (Some(name), Some(desc)) = (utf8(&orig, field.name), utf8(&orig, field.desc))
            else {
                continue;
            };
            if let Some(real) =
                own.and_then(|c| c.fields.get(&(name.to_string(), desc.to_string())))
            {
                field.name = pool.utf8(real)?;
            }
            let new = self.map_desc(desc);
            if new != desc {
                field.desc = pool.utf8(&new)?;
            }
            self.map_attrs(
                &orig,
                &mut pool,
                &mut field.attrs,
                SigKind::Field,
                &this_obf,
            )?;
        }
        for method in &mut cf.methods {
            let (Some(name), Some(desc)) = (utf8(&orig, method.name), utf8(&orig, method.desc))
            else {
                continue;
            };
            if let Some(real) =
                own.and_then(|c| c.methods.get(&(name.to_string(), desc.to_string())))
            {
                method.name = pool.utf8(real)?;
            }
            let new = self.map_desc(desc);
            if new != desc {
                method.desc = pool.utf8(&new)?;
            }
            self.map_attrs(
                &orig,
                &mut pool,
                &mut method.attrs,
                SigKind::Method,
                &this_obf,
            )?;
        }
        let mut class_attrs = std::mem::take(&mut cf.attrs);
        self.map_attrs(
            &orig,
            &mut pool,
            &mut class_attrs,
            SigKind::Class,
            &this_obf,
        )?;
        cf.attrs = class_attrs;

        cf.cp = pool.cp;
        Ok((self.map_class(&this_obf).to_string(), cf.write()))
    }

    /// The attributes of a class, field, method or record component that name classes, members or
    /// types: `kind` is what a `Signature` there is.
    fn map_attrs(
        &self,
        orig: &[Cp],
        pool: &mut Pool,
        attrs: &mut [Attr],
        kind: SigKind,
        this_obf: &str,
    ) -> Res<()> {
        for attr in attrs.iter_mut() {
            let Some(name) = utf8(orig, attr.name) else {
                continue;
            };
            match name {
                "SourceFile" if kind == SigKind::Class => {
                    if let Some(file) = self
                        .maps
                        .classes
                        .get(this_obf)
                        .and_then(|c| c.source_file.as_deref())
                    {
                        attr.data = pool.utf8(file)?.to_be_bytes().to_vec();
                    }
                }
                "Signature" => {
                    let mut r = Reader::new(&attr.data);
                    let idx = r.u2()?;
                    if let Some(sig) = utf8(orig, idx) {
                        let new = self.map_signature(sig, kind);
                        if new != sig {
                            attr.data = pool.utf8(&new)?.to_be_bytes().to_vec();
                        }
                    }
                }
                "RuntimeVisibleAnnotations" | "RuntimeInvisibleAnnotations" => {
                    let mut r = Reader::new(&attr.data);
                    let mut out = Vec::with_capacity(attr.data.len());
                    let n = r.u2()?;
                    put2(&mut out, n);
                    for _ in 0..n {
                        self.map_annotation(orig, pool, &mut r, &mut out)?;
                    }
                    attr.data = out;
                }
                "RuntimeVisibleParameterAnnotations" | "RuntimeInvisibleParameterAnnotations" => {
                    let mut r = Reader::new(&attr.data);
                    let mut out = Vec::with_capacity(attr.data.len());
                    let params = r.u1()?;
                    out.push(params);
                    for _ in 0..params {
                        let n = r.u2()?;
                        put2(&mut out, n);
                        for _ in 0..n {
                            self.map_annotation(orig, pool, &mut r, &mut out)?;
                        }
                    }
                    attr.data = out;
                }
                "AnnotationDefault" => {
                    let mut r = Reader::new(&attr.data);
                    let mut out = Vec::with_capacity(attr.data.len());
                    self.map_element(orig, pool, &mut r, &mut out)?;
                    attr.data = out;
                }
                "InnerClasses" => {
                    let mut r = Reader::new(&attr.data);
                    let mut out = Vec::with_capacity(attr.data.len());
                    let n = r.u2()?;
                    put2(&mut out, n);
                    for _ in 0..n {
                        let inner = r.u2()?;
                        let outer = r.u2()?;
                        let mut simple = r.u2()?;
                        let flags = r.u2()?;
                        if simple != 0 {
                            if let Some(obf) = class_name(orig, inner) {
                                let real = self.map_class(obf);
                                if real != obf {
                                    if let Some(s) = inner_simple_name(real) {
                                        simple = pool.utf8(s)?;
                                    }
                                }
                            }
                        }
                        put2(&mut out, inner);
                        put2(&mut out, outer);
                        put2(&mut out, simple);
                        put2(&mut out, flags);
                    }
                    attr.data = out;
                }
                "EnclosingMethod" => {
                    let mut r = Reader::new(&attr.data);
                    let class = r.u2()?;
                    let method = r.u2()?;
                    if method != 0 {
                        if let (Some(owner), Some(Cp::NameAndType(n, d))) =
                            (class_name(orig, class), orig.get(method as usize))
                        {
                            if let (Some(name), Some(desc)) = (utf8(orig, *n), utf8(orig, *d)) {
                                let new_name = self
                                    .find_member(owner, name, Some(desc), false)
                                    .unwrap_or(name);
                                let new_desc = self.map_desc(desc);
                                let nat = pool.name_and_type(new_name, &new_desc)?;
                                let mut out = Vec::new();
                                put2(&mut out, class);
                                put2(&mut out, nat);
                                attr.data = out;
                            }
                        }
                    }
                }
                "Record" => {
                    let mut r = Reader::new(&attr.data);
                    let mut out = Vec::with_capacity(attr.data.len());
                    let n = r.u2()?;
                    put2(&mut out, n);
                    for _ in 0..n {
                        let name = r.u2()?;
                        let desc = r.u2()?;
                        let mut inner = parse_attrs(&mut r)?;
                        let (mut new_name, mut new_desc) = (name, desc);
                        if let (Some(nm), Some(ds)) = (utf8(orig, name), utf8(orig, desc)) {
                            if let Some(real) = self
                                .maps
                                .classes
                                .get(this_obf)
                                .and_then(|c| c.fields.get(&(nm.to_string(), ds.to_string())))
                            {
                                new_name = pool.utf8(real)?;
                            }
                            let mapped = self.map_desc(ds);
                            if mapped != ds {
                                new_desc = pool.utf8(&mapped)?;
                            }
                        }
                        self.map_attrs(orig, pool, &mut inner, SigKind::Field, this_obf)?;
                        put2(&mut out, new_name);
                        put2(&mut out, new_desc);
                        write_attrs(&mut out, &inner);
                    }
                    attr.data = out;
                }
                "Code" => {
                    let mut r = Reader::new(&attr.data);
                    let mut out = Vec::with_capacity(attr.data.len());
                    out.extend_from_slice(r.take(4)?); // max_stack, max_locals
                    let code_len = r.u4()? as usize;
                    out.extend_from_slice(&(code_len as u32).to_be_bytes());
                    out.extend_from_slice(r.take(code_len)?);
                    let handlers = r.u2()? as usize;
                    put2(&mut out, handlers as u16);
                    out.extend_from_slice(r.take(handlers * 8)?);
                    let mut inner = parse_attrs(&mut r)?;
                    self.map_code_attrs(orig, pool, &mut inner)?;
                    write_attrs(&mut out, &inner);
                    attr.data = out;
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// The local variable tables of a method body: descriptors and signatures.
    fn map_code_attrs(&self, orig: &[Cp], pool: &mut Pool, attrs: &mut [Attr]) -> Res<()> {
        for attr in attrs.iter_mut() {
            let table = match utf8(orig, attr.name) {
                Some("LocalVariableTable") => false,
                Some("LocalVariableTypeTable") => true,
                _ => continue,
            };
            let mut r = Reader::new(&attr.data);
            let mut out = Vec::with_capacity(attr.data.len());
            let n = r.u2()?;
            put2(&mut out, n);
            for _ in 0..n {
                let (start, len, name, ty, index) = (r.u2()?, r.u2()?, r.u2()?, r.u2()?, r.u2()?);
                let mut new_ty = ty;
                if let Some(text) = utf8(orig, ty) {
                    let new = if table {
                        self.map_signature(text, SigKind::Field)
                    } else {
                        self.map_desc(text)
                    };
                    if new != text {
                        new_ty = pool.utf8(&new)?;
                    }
                }
                for v in [start, len, name, new_ty, index] {
                    put2(&mut out, v);
                }
            }
            attr.data = out;
        }
        Ok(())
    }

    fn map_annotation(
        &self,
        orig: &[Cp],
        pool: &mut Pool,
        r: &mut Reader,
        out: &mut Vec<u8>,
    ) -> Res<()> {
        let ty = r.u2()?;
        let annotation_class = utf8(orig, ty).and_then(desc_class);
        match utf8(orig, ty) {
            Some(d) => {
                let new = self.map_desc(d);
                put2(out, if new != d { pool.utf8(&new)? } else { ty });
            }
            None => put2(out, ty),
        }
        let pairs = r.u2()?;
        put2(out, pairs);
        for _ in 0..pairs {
            let name = r.u2()?;
            let mut new_name = name;
            if let (Some(class), Some(n)) = (annotation_class, utf8(orig, name)) {
                if let Some(real) = self.find_member(class, n, None, false) {
                    new_name = pool.utf8(real)?;
                }
            }
            put2(out, new_name);
            self.map_element(orig, pool, r, out)?;
        }
        Ok(())
    }

    fn map_element(
        &self,
        orig: &[Cp],
        pool: &mut Pool,
        r: &mut Reader,
        out: &mut Vec<u8>,
    ) -> Res<()> {
        let tag = r.u1()?;
        out.push(tag);
        match tag {
            b'B' | b'C' | b'D' | b'F' | b'I' | b'J' | b'S' | b'Z' | b's' => put2(out, r.u2()?),
            b'e' => {
                let ty = r.u2()?;
                let constant = r.u2()?;
                let mut new_ty = ty;
                let mut new_constant = constant;
                if let Some(d) = utf8(orig, ty) {
                    if let (Some(class), Some(n)) = (desc_class(d), utf8(orig, constant)) {
                        if let Some(real) = self.find_member(class, n, Some(d), true) {
                            new_constant = pool.utf8(real)?;
                        }
                    }
                    let new = self.map_desc(d);
                    if new != d {
                        new_ty = pool.utf8(&new)?;
                    }
                }
                put2(out, new_ty);
                put2(out, new_constant);
            }
            b'c' => {
                let class = r.u2()?;
                match utf8(orig, class) {
                    Some(d) => {
                        let new = self.map_desc(d);
                        put2(out, if new != d { pool.utf8(&new)? } else { class });
                    }
                    None => put2(out, class),
                }
            }
            b'@' => self.map_annotation(orig, pool, r, out)?,
            b'[' => {
                let n = r.u2()?;
                put2(out, n);
                for _ in 0..n {
                    self.map_element(orig, pool, r, out)?;
                }
            }
            other => return Err(format!("Unknown annotation value tag {other}")),
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------- jars

/// Remaps every class of `input`, writing the result to `output` (the classes under their real
/// names, other files as they are). The jar's signature files and the digests of its manifest are
/// dropped: they no longer hold.
pub fn remap_jar(input: &Path, mappings: &Mappings, output: &Path) -> Res<Stats> {
    let file = File::open(input).map_err(|e| format!("{}: {e}", input.display()))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| format!("{}: {e}", input.display()))?;
    let mut entries: Vec<(String, Vec<u8>)> = Vec::with_capacity(archive.len());
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        if entry.is_dir() {
            continue;
        }
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        entries.push((entry.name().to_string(), bytes));
    }
    let classes: Vec<&[u8]> = entries
        .iter()
        .filter(|(n, _)| n.ends_with(".class"))
        .map(|(_, b)| b.as_slice())
        .collect();
    let remapper = Remapper::new(mappings, &classes)?;

    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let partial = output.with_extension("jar.partial");
    let mut zip = zip::ZipWriter::new(File::create(&partial).map_err(|e| e.to_string())?);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut stats = Stats::default();
    for (name, bytes) in &entries {
        let upper = name.to_ascii_uppercase();
        if upper.starts_with("META-INF/")
            && (upper.ends_with(".SF") || upper.ends_with(".RSA") || upper.ends_with(".DSA"))
        {
            continue;
        }
        let (path, bytes) = if name.ends_with(".class") {
            let (real, bytes) = remapper
                .remap_class(bytes)
                .map_err(|e| format!("{name}: {e}"))?;
            stats.classes += 1;
            let path = format!("{real}.class");
            if &path != name {
                stats.renamed_classes += 1;
            }
            (path, bytes)
        } else if upper == "META-INF/MANIFEST.MF" {
            stats.resources += 1;
            (name.clone(), b"Manifest-Version: 1.0\r\n\r\n".to_vec())
        } else {
            stats.resources += 1;
            (name.clone(), bytes.clone())
        };
        zip.start_file(path, options).map_err(|e| e.to_string())?;
        zip.write_all(&bytes).map_err(|e| e.to_string())?;
    }
    zip.finish().map_err(|e| e.to_string())?;
    std::fs::rename(&partial, output).map_err(|e| e.to_string())?;
    Ok(stats)
}

/// Reads ProGuard mapping files of the jar's own side (Mojang's `client.txt` for the client jar,
/// `server.txt` for the server's game jar) and remaps `input` into `output`.
pub fn remap_files(input: &Path, mapping_files: &[&Path], output: &Path) -> Res<Stats> {
    let mut mappings = Mappings::default();
    for file in mapping_files {
        let text = std::fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
        mappings
            .add(&text)
            .map_err(|e| format!("{}: {e}", file.display()))?;
    }
    if mappings.is_empty() {
        return Err("No mapping file given".into());
    }
    remap_jar(input, &mappings, output)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAPPING: &str = r#"# (c) Mojang
net.minecraft.core.BlockPos -> a:
# {"fileName":"BlockPos.java","id":"sourceFile"}
    int x -> b
    1:1:net.minecraft.core.BlockPos offset(int,int[]) -> c
    42:42:void inlined.Other.thing():7:7 -> d
net.minecraft.core.Holder -> e:
    net.minecraft.core.BlockPos pos -> f
    java.util.List all(net.minecraft.core.BlockPos[]) -> g
net.minecraft.core.Holder$Ref -> e$a:
"#;

    fn mappings() -> Mappings {
        Mappings::parse(MAPPING).unwrap()
    }

    #[test]
    fn a_proguard_mapping_is_read_with_obfuscated_descriptors() {
        let m = mappings();
        assert_eq!(m.len(), 3);
        let pos = &m.classes["a"];
        assert_eq!(pos.name, "net/minecraft/core/BlockPos");
        assert_eq!(pos.source_file.as_deref(), Some("BlockPos.java"));
        assert_eq!(pos.fields[&("b".into(), "I".into())], "x");
        // The argument and return types are the obfuscated ones, and the inlined frame is skipped.
        assert_eq!(pos.methods[&("c".into(), "(I[I)La;".into())], "offset");
        assert_eq!(pos.methods.len(), 1);
        let holder = &m.classes["e"];
        assert_eq!(holder.fields[&("f".into(), "La;".into())], "pos");
        assert_eq!(
            holder.methods[&("g".into(), "([La;)Ljava/util/List;".into())],
            "all"
        );
    }

    #[test]
    fn descriptors_and_signatures_map_class_names_only() {
        let m = mappings();
        let r = Remapper::new(&m, &[]).unwrap();
        assert_eq!(
            r.map_desc("(La;I[[Le;Ljava/lang/String;)V"),
            "(Lnet/minecraft/core/BlockPos;I[[Lnet/minecraft/core/Holder;Ljava/lang/String;)V"
        );
        assert_eq!(
            r.map_signature("<T:Ljava/lang/Object;>Le<TT;>.a;", SigKind::Class),
            "<T:Ljava/lang/Object;>Lnet/minecraft/core/Holder<TT;>.Ref;"
        );
        assert_eq!(
            r.map_signature("<T:La;>(Ljava/util/List<+La;>;)La;^Ljava/io/IOException;", SigKind::Method),
            "<T:Lnet/minecraft/core/BlockPos;>(Ljava/util/List<+Lnet/minecraft/core/BlockPos;>;)Lnet/minecraft/core/BlockPos;^Ljava/io/IOException;"
        );
        assert_eq!(
            r.map_signature(
                "Ljava/lang/Object;Ljava/lang/Comparable<La;>;",
                SigKind::Class
            ),
            "Ljava/lang/Object;Ljava/lang/Comparable<Lnet/minecraft/core/BlockPos;>;"
        );
        // Not a signature: left alone.
        assert_eq!(r.map_signature("(((", SigKind::Method), "(((");
        assert_eq!(inner_simple_name("a/b$1Foo"), Some("Foo"));
        assert_eq!(inner_simple_name("a/b"), None);
    }

    /// A class `a` with a field `b`, a method `c`, and a method `h` that calls `c` through its
    /// subclass `i`, built by hand: the references and the declarations all come out renamed, the
    /// bytecode is untouched, and the result parses again.
    #[test]
    fn a_class_is_renamed_through_its_references() {
        let m = mappings();
        // Constant pool: 1 class a, 2 class i, 3 Utf8 "c", 4 Utf8 "(I[I)La;", 5 NaT, 6 Methodref
        // i.c, 7 Utf8 "b", 8 Utf8 "I", 9 Utf8 "h", 10 Utf8 "()V", 11 Utf8 "Code", 12.. names.
        let mut cp = vec![Cp::Unusable];
        let utf = |s: &str| Cp::Utf8(s.as_bytes().to_vec());
        cp.push(Cp::Class(13)); // 1
        cp.push(Cp::Class(14)); // 2
        cp.push(utf("c")); // 3
        cp.push(utf("(I[I)La;")); // 4
        cp.push(Cp::NameAndType(3, 4)); // 5
        cp.push(Cp::Method(2, 5)); // 6
        cp.push(utf("b")); // 7
        cp.push(utf("I")); // 8
        cp.push(utf("h")); // 9
        cp.push(utf("()V")); // 10
        cp.push(utf("Code")); // 11
        cp.push(utf("Object")); // 12
        cp.push(utf("a")); // 13
        cp.push(utf("i")); // 14
        let code = vec![0u8, 1, 0, 1, 0, 0, 0, 1, 0xb1, 0, 0, 0, 0];
        let cf = ClassFile {
            minor: 0,
            major: 65,
            cp,
            access: 0x21,
            this: 1,
            super_class: 0,
            interfaces: vec![],
            fields: vec![Member {
                access: 1,
                name: 7,
                desc: 8,
                attrs: vec![],
            }],
            methods: vec![Member {
                access: 1,
                name: 9,
                desc: 10,
                attrs: vec![Attr {
                    name: 11,
                    data: code.clone(),
                }],
            }],
            attrs: vec![],
        };
        let bytes = cf.write();
        // `i` extends `a`, which declares `c`.
        let sub = ClassFile {
            minor: 0,
            major: 65,
            cp: vec![Cp::Unusable, Cp::Class(2), utf("i"), Cp::Class(4), utf("a")],
            access: 0x21,
            this: 1,
            super_class: 3,
            interfaces: vec![],
            fields: vec![],
            methods: vec![],
            attrs: vec![],
        }
        .write();
        let r = Remapper::new(&m, &[&bytes, &sub]).unwrap();
        let (name, out) = r.remap_class(&bytes).unwrap();
        assert_eq!(name, "net/minecraft/core/BlockPos");
        let back = ClassFile::parse(&out).unwrap();
        let string = |i: u16| utf8(&back.cp, i).unwrap().to_string();
        assert_eq!(string(back.fields[0].name), "x");
        assert_eq!(string(back.methods[0].name), "h");
        assert_eq!(back.methods[0].attrs[0].data, code);
        let Cp::Method(class, nat) = back.cp[6] else {
            panic!()
        };
        assert_eq!(class_name(&back.cp, class), Some("i"));
        let Cp::NameAndType(n, d) = back.cp[nat as usize] else {
            panic!()
        };
        assert_eq!(string(n), "offset");
        assert_eq!(string(d), "(I[I)Lnet/minecraft/core/BlockPos;");
        assert_eq!(
            class_name(&back.cp, back.this),
            Some("net/minecraft/core/BlockPos")
        );
        // Nothing but strings and name-and-types were added: the old entries are all still there.
        assert!(back.cp.len() >= 14);
    }
}
