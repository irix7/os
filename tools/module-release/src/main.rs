// SPDX-License-Identifier: GPL-3.0-only
//! Wrap flat module images with manifest v2 footers, sign them, and emit the
//! release package index and trust policy.
//!
//! The tool reads one TOML spec. Module images must already be flattened by
//! `llvm-objcopy`; the ELF is read only for entry and export symbol offsets.
//! Everything it writes is deterministic for a fixed key and input set, except
//! signatures (Ed25519 is deterministic too, so the whole output is stable).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use ed25519_dalek::{Signer, SigningKey};
use module_resolver::footer;
use module_resolver::index::PackageIndex;
use module_resolver::runtime::Plan;
use module_resolver::{Abi, Dependency, Export, Manifest};
use module_trust::{image_message, SignatureRecord, TrustPolicy};
use object::{Object, ObjectSymbol};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};

#[derive(Debug, Deserialize)]
struct Spec {
    version: u16,
    release: Release,
    #[serde(default, rename = "module")]
    modules: Vec<ModuleSpec>,
}

#[derive(Debug, Deserialize)]
struct Release {
    index: String,
    trust: String,
    index_signature: String,
    #[serde(default = "default_true")]
    require_signatures: bool,
    #[serde(default)]
    trust_epoch: u32,
    #[serde(default)]
    revoked: Vec<u32>,
    #[serde(default)]
    root: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ModuleSpec {
    name: String,
    version: String,
    abi: String,
    #[serde(default)]
    region: Option<u32>,
    load_address: u64,
    elf: PathBuf,
    flat: PathBuf,
    image: String,
    url: String,
    #[serde(default = "default_entry")]
    entry: String,
    #[serde(default)]
    max_size: u64,
    #[serde(default = "default_true")]
    index: bool,
    #[serde(default)]
    depends: Vec<String>,
    #[serde(default)]
    exports: Vec<ExportSpec>,
}

#[derive(Debug, Deserialize)]
struct ExportSpec {
    interface: String,
    version: u16,
    symbol: String,
}

fn default_true() -> bool {
    true
}

fn default_entry() -> String {
    "_start".to_string()
}

struct Built {
    name: String,
    version: String,
    abi: Abi,
    region: Option<u32>,
    url: String,
    digest: [u8; 32],
    signature: Option<SignatureRecord>,
    dependencies: Vec<Dependency>,
    exports: Vec<Export>,
    image: PathBuf,
    size: usize,
    indexed: bool,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("module-release: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("build") => build(parse_build_args(args)?),
        Some("pubkey") => pubkey(parse_pubkey_args(args)?),
        Some("trust") => trust(parse_trust_args(args)?),
        Some("--help") | Some("-h") | None => {
            usage();
            Ok(())
        }
        Some(other) => Err(format!("unknown command {other}")),
    }
}

struct BuildArgs {
    spec: PathBuf,
    out: PathBuf,
    raw_base: String,
    release_version: String,
    key: Option<[u8; 32]>,
    epoch: Option<u32>,
    allow_unsigned: bool,
    target_dir: String,
}

fn parse_build_args(mut args: impl Iterator<Item = String>) -> Result<BuildArgs, String> {
    let mut spec = None;
    let mut out = None;
    let mut raw_base = String::from("https://raw.githubusercontent.com/irix7/os/main");
    let mut release_version = String::from("dev");
    let mut key = None;
    let mut epoch = None;
    let mut allow_unsigned = false;
    let mut target_dir = String::from("target");
    while let Some(arg) = args.next() {
        let value = |args: &mut dyn Iterator<Item = String>| {
            args.next().ok_or_else(|| format!("{arg} needs a value"))
        };
        match arg.as_str() {
            "--spec" => spec = Some(PathBuf::from(value(&mut args)?)),
            "--out" => out = Some(PathBuf::from(value(&mut args)?)),
            "--raw-base" => raw_base = value(&mut args)?,
            "--release" => release_version = value(&mut args)?,
            "--key" => key = Some(parse_seed(&value(&mut args)?)?),
            "--epoch" => {
                epoch = Some(
                    value(&mut args)?
                        .parse()
                        .map_err(|_| String::from("bad --epoch"))?,
                )
            }
            "--allow-unsigned" => allow_unsigned = true,
            "--target-dir" => target_dir = value(&mut args)?,
            other => return Err(format!("unknown build option {other}")),
        }
    }
    Ok(BuildArgs {
        spec: spec.ok_or("--spec is required")?,
        out: out.ok_or("--out is required")?,
        raw_base,
        release_version,
        key,
        epoch,
        allow_unsigned,
        target_dir,
    })
}

struct PubkeyArgs {
    seed: [u8; 32],
}

fn parse_pubkey_args(mut args: impl Iterator<Item = String>) -> Result<PubkeyArgs, String> {
    let mut seed = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--seed" => {
                let value = args.next().ok_or("--seed needs a value")?;
                seed = Some(parse_seed(&value)?);
            }
            other => return Err(format!("unknown pubkey option {other}")),
        }
    }
    Ok(PubkeyArgs {
        seed: seed.ok_or("--seed is required")?,
    })
}

struct TrustArgs {
    seed: Option<[u8; 32]>,
    epoch: u32,
    out: PathBuf,
    unsigned: bool,
    revoked: Vec<u32>,
}

fn parse_trust_args(mut args: impl Iterator<Item = String>) -> Result<TrustArgs, String> {
    let mut seed = None;
    let mut epoch = 1;
    let mut out = None;
    let mut unsigned = false;
    let mut revoked = Vec::new();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--seed" => {
                let value = args.next().ok_or("--seed needs a value")?;
                seed = Some(parse_seed(&value)?);
            }
            "--epoch" => {
                epoch = args
                    .next()
                    .ok_or("--epoch needs a value")?
                    .parse()
                    .map_err(|_| String::from("bad --epoch"))?;
            }
            "--out" => out = Some(PathBuf::from(args.next().ok_or("--out needs a value")?)),
            "--unsigned" => unsigned = true,
            "--revoke" => {
                revoked.push(
                    args.next()
                        .ok_or("--revoke needs a value")?
                        .parse()
                        .map_err(|_| String::from("bad --revoke"))?,
                );
            }
            other => return Err(format!("unknown trust option {other}")),
        }
    }
    if !unsigned && seed.is_none() {
        return Err(String::from("signed trust policy needs --seed"));
    }
    Ok(TrustArgs {
        seed,
        epoch,
        out: out.ok_or("--out is required")?,
        unsigned,
        revoked,
    })
}

fn usage() {
    eprintln!(
        "usage:\n  module-release build --spec FILE --out DIR [--raw-base URL] [--release TAG] [--key 64HEX] [--epoch N] [--allow-unsigned] [--target-dir DIR]\n  module-release pubkey --seed 64HEX\n  module-release trust --out FILE [--seed 64HEX] [--epoch N] [--unsigned] [--revoke N]"
    );
}

fn trust(args: TrustArgs) -> Result<(), String> {
    let signing = args.seed.map(|seed| SigningKey::from_bytes(&seed));
    let policy = TrustPolicy {
        current_epoch: args.epoch,
        require_signatures: !args.unsigned,
        keys: signing
            .as_ref()
            .map(|key| vec![(args.epoch, key.verifying_key())])
            .unwrap_or_default(),
        revoked: args.revoked,
    };
    let text = policy.to_text();
    TrustPolicy::parse(&text)
        .map_err(|error| format!("generated trust policy is invalid: {error}"))?;
    fs::write(&args.out, text).map_err(|error| format!("write {}: {error}", args.out.display()))?;
    println!(
        "trust: {} ({})",
        args.out.display(),
        if args.unsigned {
            "unsigned mode declared"
        } else {
            "signatures required"
        }
    );
    Ok(())
}

fn pubkey(args: PubkeyArgs) -> Result<(), String> {
    let key = SigningKey::from_bytes(&args.seed);
    println!("{}", hex(key.verifying_key().as_bytes()));
    Ok(())
}

fn parse_seed(text: &str) -> Result<[u8; 32], String> {
    let bytes = unhex(text).ok_or("key is not 64 hex digits")?;
    if bytes.len() != 32 {
        return Err(String::from("key is not 32 bytes"));
    }
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&bytes);
    Ok(seed)
}

fn build(args: BuildArgs) -> Result<(), String> {
    let text = fs::read_to_string(&args.spec)
        .map_err(|error| format!("read {}: {error}", args.spec.display()))?;
    let spec: Spec = toml::from_str(&text).map_err(|error| format!("parse spec: {error}"))?;
    if spec.version != 1 {
        return Err(format!("unsupported spec version {}", spec.version));
    }
    let require_signatures = spec.release.require_signatures && !args.allow_unsigned;
    if require_signatures && args.key.is_none() {
        return Err(String::from(
            "signatures are required; pass --key or explicitly --allow-unsigned",
        ));
    }
    let signed = args.key.is_some() && require_signatures;
    let epoch = args.epoch.unwrap_or(spec.release.trust_epoch);
    let signing = args.key.map(|seed| SigningKey::from_bytes(&seed));

    fs::create_dir_all(&args.out)
        .map_err(|error| format!("create {}: {error}", args.out.display()))?;

    let mut built: BTreeMap<(String, String), Built> = BTreeMap::new();
    for module in &spec.modules {
        let key = (module.name.clone(), module.version.clone());
        if built.contains_key(&key) {
            return Err(format!(
                "duplicate module {}@{}",
                module.name, module.version
            ));
        }
        let abi = parse_abi(&module.abi)?;
        if module.index && module.region.is_none() {
            return Err(format!(
                "{} is indexed but has no arena region",
                module.name
            ));
        }
        let elf = resolve_path(&module.elf, &args.target_dir);
        let flat_path = resolve_path(&module.flat, &args.target_dir);
        let addresses = symbols(&elf)?;
        let entry = resolve_symbol(&addresses, &module.entry, module.load_address)?;
        let mut exports = Vec::new();
        for export in &module.exports {
            let offset = resolve_symbol(&addresses, &export.symbol, module.load_address)?;
            if offset % 4 != 0 {
                return Err(format!(
                    "{} export {} offset {offset} is not 4-byte aligned",
                    module.name, export.symbol
                ));
            }
            exports
                .push(Export::new(export.interface.as_str(), export.version).with_offset(offset));
        }
        let mut dependencies = Vec::new();
        for dependency in &module.depends {
            let (name, version) = dependency
                .split_once('@')
                .ok_or_else(|| format!("bad dependency {dependency}"))?;
            let dep = built
                .get(&(name.to_string(), version.to_string()))
                .ok_or_else(|| {
                    format!(
                        "{} depends on {}@{}, which must be built first",
                        module.name, name, version
                    )
                })?;
            dependencies.push(Dependency::new(name, version, dep.digest));
        }
        let flat = fs::read(&flat_path)
            .map_err(|error| format!("read {}: {error}", flat_path.display()))?;
        let mut manifest = Manifest::new(module.name.as_str(), module.version.as_str(), abi)
            .with_entry_offset(entry);
        for dependency in &dependencies {
            manifest = manifest.with_dependency(dependency.clone());
        }
        for export in &exports {
            manifest = manifest.with_export(export.clone());
        }
        let image = footer::wrap(&flat, &manifest)
            .map_err(|error| format!("wrap {}: {error}", module.name))?;
        if module.max_size != 0 && image.len() as u64 > module.max_size {
            return Err(format!(
                "{} image is {} bytes, over the {} byte limit",
                module.name,
                image.len(),
                module.max_size
            ));
        }
        let digest = sha256(&image);
        let signature = signing
            .as_ref()
            .filter(|_| signed)
            .map(|key| SignatureRecord::new(epoch, key.sign(&image_message(&digest)).to_bytes()));
        let path = args.out.join(&module.image);
        fs::write(&path, &image).map_err(|error| format!("write {}: {error}", path.display()))?;
        if let Some(signature) = &signature {
            let sig_path = args.out.join(format!("{}.sig", module.image));
            fs::write(&sig_path, signature.to_text())
                .map_err(|error| format!("write {}: {error}", sig_path.display()))?;
        }
        let url = substitute(&module.url, &args.raw_base, &args.release_version);
        built.insert(
            key,
            Built {
                name: module.name.clone(),
                version: module.version.clone(),
                abi,
                region: module.region,
                url,
                digest,
                signature,
                dependencies,
                exports,
                image: path,
                size: image.len(),
                indexed: module.index,
            },
        );
    }

    let mut regions = BTreeMap::new();
    for module in built.values().filter(|module| module.indexed) {
        if let Some(region) = module.region {
            if let Some(other) = regions.insert(region, module.name.as_str()) {
                return Err(format!(
                    "{} and {} both declare arena region {region}",
                    other, module.name
                ));
            }
        }
    }

    let mut lines = String::from(
        "# IRIX 7 runtime package index, version 2.\n\
         # Generated by tools/module-release. Records carry exact depends=,\n\
         # abi=, region=, exports= and sig= fields.\n\
         version=2\n",
    );
    for module in built.values().filter(|module| module.indexed) {
        let mut line = format!(
            "module {} {} {} {} abi={}.{}",
            module.name,
            module.version,
            module.url,
            hex(&module.digest),
            module.abi.major,
            module.abi.minor
        );
        if let Some(region) = module.region {
            line.push_str(&format!(" region={region}"));
        }
        if !module.dependencies.is_empty() {
            line.push_str(" depends=");
            for (index, dependency) in module.dependencies.iter().enumerate() {
                if index > 0 {
                    line.push(',');
                }
                line.push_str(&format!(
                    "{}@{}:{}",
                    dependency.module.name,
                    dependency.module.version,
                    hex(&dependency.sha256)
                ));
            }
        }
        if !module.exports.is_empty() {
            line.push_str(" exports=");
            for (index, export) in module.exports.iter().enumerate() {
                if index > 0 {
                    line.push(',');
                }
                line.push_str(&format!(
                    "{}@{}:{}",
                    export.interface, export.version, export.offset
                ));
            }
        }
        if let Some(signature) = &module.signature {
            line.push_str(" sig=");
            line.push_str(&signature.to_text());
        }
        line.push_str(" runtime module");
        lines.push_str(&line);
        lines.push('\n');
    }
    let index_path = args.out.join(&spec.release.index);
    fs::write(&index_path, &lines)
        .map_err(|error| format!("write {}: {error}", index_path.display()))?;
    let index_digest = sha256(lines.as_bytes());
    if let Some(key) = &signing {
        if signed {
            let signature =
                SignatureRecord::new(epoch, key.sign(&image_message(&index_digest)).to_bytes());
            let sig_path = args.out.join(&spec.release.index_signature);
            fs::write(&sig_path, signature.to_text())
                .map_err(|error| format!("write {}: {error}", sig_path.display()))?;
        }
    }

    let policy = if require_signatures {
        let key = signing.as_ref().ok_or("signed policy needs a key")?;
        let mut policy = TrustPolicy::new(epoch, key.verifying_key(), true);
        policy.revoked = spec.release.revoked.clone();
        policy
    } else {
        TrustPolicy {
            current_epoch: epoch,
            require_signatures: false,
            keys: signing
                .as_ref()
                .map(|key| vec![(epoch, key.verifying_key())])
                .unwrap_or_default(),
            revoked: spec.release.revoked.clone(),
        }
    };
    let policy_path = args.out.join(&spec.release.trust);
    let policy_text = policy.to_text();
    TrustPolicy::parse(&policy_text)
        .map_err(|error| format!("generated trust policy is invalid: {error}"))?;
    fs::write(&policy_path, policy_text)
        .map_err(|error| format!("write {}: {error}", policy_path.display()))?;

    // Self-check the generated index and manifest footers.
    let index = PackageIndex::parse(lines.as_bytes())
        .map_err(|error| format!("generated index does not parse: {error}"))?;
    for module in built.values() {
        let image = fs::read(&module.image)
            .map_err(|error| format!("read {}: {error}", module.image.display()))?;
        let (manifest, _) = footer::parse(&image)
            .map_err(|error| format!("{} footer: {error}", module.name))?
            .ok_or_else(|| format!("{} has no footer", module.name))?;
        if manifest.id.name != module.name || manifest.id.version != module.version {
            return Err(format!("{} manifest identity is wrong", module.name));
        }
        if module.indexed {
            let record = index
                .find_module(&module.name, &module.version)
                .ok_or_else(|| {
                    format!("generated index lost {}@{}", module.name, module.version)
                })?;
            if record.sha256 != module.digest {
                return Err(format!("{} digest mismatch in index", module.name));
            }
            let expected = record
                .manifest()
                .ok_or_else(|| format!("{} index record has no ABI", module.name))?;
            if manifest.abi != expected.abi
                || manifest.dependencies != expected.dependencies
                || manifest.exports != expected.exports
            {
                return Err(format!("{} manifest does not match the index", module.name));
            }
        }
    }
    if let Some(root) = &spec.release.root {
        let (name, version) = root
            .split_once('@')
            .ok_or_else(|| format!("bad release root {root}"))?;
        let plan = Plan::from_index(&index, name, version, Abi::new(12, 0))
            .map_err(|error| error.status_text(None))?;
        println!(
            "plan: {} nodes, root={}@{}",
            plan.nodes.len(),
            plan.root.name,
            plan.root.version
        );
    }

    println!(
        "index: {} ({} bytes, sha256 {})",
        index_path.display(),
        lines.len(),
        hex(&index_digest)
    );
    println!(
        "trust: {} ({})",
        policy_path.display(),
        if require_signatures {
            "signatures required"
        } else {
            "unsigned mode declared"
        }
    );
    for module in built.values() {
        println!(
            "module: {}@{} region={} image={} size={} sha256={}",
            module.name,
            module.version,
            module
                .region
                .map(|region| region.to_string())
                .unwrap_or_else(|| "-".to_string()),
            module.image.display(),
            module.size,
            hex(&module.digest)
        );
    }
    Ok(())
}

fn substitute(template: &str, raw_base: &str, version: &str) -> String {
    template
        .replace("{raw_base}", raw_base)
        .replace("{version}", version)
}

fn resolve_path(template: &Path, target_dir: &str) -> PathBuf {
    PathBuf::from(template.to_string_lossy().replace("{target}", target_dir))
}

fn parse_abi(text: &str) -> Result<Abi, String> {
    let (major, minor) = text
        .split_once('.')
        .ok_or_else(|| format!("bad ABI {text}"))?;
    Ok(Abi::new(
        major.parse().map_err(|_| format!("bad ABI {text}"))?,
        minor.parse().map_err(|_| format!("bad ABI {text}"))?,
    ))
}

fn symbols(path: &Path) -> Result<BTreeMap<String, u64>, String> {
    let data = fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    let file = object::File::parse(&*data)
        .map_err(|error| format!("parse {}: {error}", path.display()))?;
    let mut out = BTreeMap::new();
    for symbol in file.symbols() {
        if let Ok(name) = symbol.name() {
            out.insert(name.to_string(), symbol.address());
        }
    }
    Ok(out)
}

fn resolve_symbol(
    symbols: &BTreeMap<String, u64>,
    name: &str,
    load_address: u64,
) -> Result<u32, String> {
    let address = symbols
        .get(name)
        .ok_or_else(|| format!("missing symbol {name}"))?;
    let offset = address
        .checked_sub(load_address)
        .ok_or_else(|| format!("symbol {name} is below the load address"))?;
    u32::try_from(offset).map_err(|_| format!("symbol {name} offset overflows u32"))
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    let digest = Sha256::digest(bytes);
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(text.len() / 2);
    for index in 0..text.len() / 2 {
        let hi = hex_digit(text.as_bytes()[index * 2])?;
        let lo = hex_digit(text.as_bytes()[index * 2 + 1])?;
        out.push((hi << 4) | lo);
    }
    Some(out)
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
