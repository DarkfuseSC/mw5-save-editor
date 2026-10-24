use mw5save::prop::{ArrayBody, PropertyList, StructBody, Value};
use mw5save::save::{Archive, SaveFile};
use std::path::{Path, PathBuf};

fn usage() -> ! {
    eprintln!("mw5save <command> <path>\n  roundtrip <file|dir>        parse every level, reserialize, verify byte-identical\n  diagnose  <file|dir>        classify damage (truncation, size mismatch, archive length, opaque structs)\n  models    <file>            list models in PersistentModelData\n  dump      <file> [model]    print property tree (top level, or one model)\n  corrupt   <kind> <in> <out> write one damaged copy (kinds: {})\n  fixtures  <in> <outdir>     write every corruption kind and diagnose each", mw5save::corrupt::KINDS.join(", "));
    std::process::exit(2);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        usage();
    }
    match args[1].as_str() {
        "roundtrip" => cmd_roundtrip(Path::new(&args[2])),
        "diagnose" => cmd_diagnose(Path::new(&args[2])),
        "models" => cmd_models(Path::new(&args[2])),
        "dump" => cmd_dump(Path::new(&args[2]), args.get(3).map(|s| s.as_str())),
        "corrupt" if args.len() >= 5 => cmd_corrupt(&args[2], Path::new(&args[3]), Path::new(&args[4])),
        "fixtures" if args.len() >= 4 => cmd_fixtures(Path::new(&args[2]), Path::new(&args[3])),
        "repair" if args.len() >= 4 => cmd_repair(Path::new(&args[2]), Path::new(&args[3])),
        "repair-all" if args.len() >= 4 => cmd_repair_all(Path::new(&args[2]), Path::new(&args[3])),
        "find" if args.len() >= 4 => cmd_find(Path::new(&args[2]), &args[3]),
        "actors" => cmd_actors(Path::new(&args[2]), args.get(3).map(|s| s.as_str())),
        "coyote" => cmd_coyote(Path::new(&args[2]), &args[3..]),
        _ => usage(),
    }
}

/// `coyote <file>` lists Coyote's Mission Pack entries.
/// `coyote <file> <id> <int> <out>` sets one entry's int slot and writes a copy,
/// then re-opens the copy and verifies the value and that it diagnoses clean.
fn cmd_coyote(path: &Path, rest: &[String]) {
    use mw5save::campaign::Campaign;
    let data = std::fs::read(path).unwrap();
    let mut c = Campaign::open(&data).unwrap();
    // Sanity: the Campaign layer (wrappers + actors parsed) must still round-trip untouched.
    let rt = c.to_bytes() == data;
    println!("campaign-layer roundtrip: {}", if rt { "identical" } else { "MISMATCH" });
    let entries = c.coyote_entries();
    if rest.len() < 3 {
        for e in &entries {
            println!("{:<3} {:<44} str={:<20} f={:<8} b={:<5} i={}", e.index, e.id, e.string_val, e.float_val, e.bool_val, e.int_val);
        }
        println!("--- {} entries", entries.len());
        return;
    }
    let (id, val, out) = (&rest[0], rest[1].parse::<i32>().expect("int"), Path::new(&rest[2]));
    let Some(e) = entries.iter().find(|e| &e.id == id) else { eprintln!("no entry {}", id); std::process::exit(1) };
    let ok = c.set_coyote_entry(e.index, &e.string_val, e.float_val, e.bool_val, val);
    println!("set {} int {} -> {}: {}", id, e.int_val, val, ok);
    let bytes = c.to_bytes();
    std::fs::write(out, &bytes).unwrap();
    let rep = mw5save::diagnose::diagnose(&bytes);
    let back = Campaign::open(&bytes).unwrap().coyote_entries().into_iter().find(|x| &x.id == id).unwrap();
    println!("wrote {} ({} bytes, {}), re-read int = {}", out.display(), bytes.len(), if rep.fatal() { "DAMAGED" } else { "clean" }, back.int_val);
    if rep.fatal() || back.int_val != val { std::process::exit(1); }
}

/// List SaveStateModel.PersistentActors; with a class substring, decode that
/// actor's ByteData archive and dump it.
fn cmd_actors(path: &Path, which: Option<&str>) {
    let data = std::fs::read(path).unwrap();
    let mut sf = SaveFile::parse(&data).unwrap();
    let m = sf.model("SaveStateModel").unwrap();
    let Some(els) = m.props.get("PersistentActors").and_then(|p| p.as_struct_array()) else { println!("no PersistentActors"); return };
    for (i, el) in els.iter().enumerate() {
        let class = el.get("ActorClass").and_then(|p| p.as_fstring()).unwrap_or_default();
        let bd = el.get("ByteData").and_then(|p| match &p.value { Value::Array { body: ArrayBody::Raw(b), .. } => Some(b), _ => None });
        let len = bd.map(|b| b.len()).unwrap_or(0);
        match which {
            None => println!("[{}] {} bytes  {}", i, len, class),
            Some(w) if class.to_lowercase().contains(&w.to_lowercase()) => {
                println!("[{}] {}", i, class);
                match bd.map(|b| Archive::parse(b)) {
                    Some(Ok(arc)) => { dump_list(&arc.props, 1, 6); println!("  footer {} bytes", arc.footer.len()); }
                    Some(Err(e)) => println!("  ByteData is not a nested archive: {}", e),
                    None => println!("  no ByteData"),
                }
            }
            _ => {}
        }
    }
}

fn value_text(p: &mw5save::prop::Property) -> String {
    match p.ptype.as_str() {
        "IntProperty" => p.as_i32().map(|v| v.to_string()).unwrap_or_default(),
        "Int64Property" => p.as_i64().map(|v| v.to_string()).unwrap_or_default(),
        "FloatProperty" => p.as_f32().map(|v| v.to_string()).unwrap_or_default(),
        "StrProperty" | "NameProperty" | "ObjectProperty" | "EnumProperty" | "ByteProperty" => p.as_fstring().unwrap_or_default(),
        "BoolProperty" => match &p.value { Value::Bool { value, .. } => (*value != 0).to_string(), _ => String::new() },
        "TextProperty" => p.payload().map(mw5save::campaign::read_text).unwrap_or_default(),
        _ => String::new(),
    }
}

fn find_in(pl: &PropertyList, path: &str, needle: &str, hits: &mut Vec<String>) {
    for p in &pl.props {
        let ppath = format!("{}.{}", path, p.name);
        let txt = value_text(p);
        let struct_name = match &p.value { Value::Struct { struct_name, .. } => struct_name.as_str(), _ => "" };
        if ppath.to_lowercase().contains(needle) || txt.to_lowercase().contains(needle) || struct_name.to_lowercase().contains(needle) {
            hits.push(format!("{} : {}{} {}", ppath, p.ptype, if struct_name.is_empty() { String::new() } else { format!("<{}>", struct_name) }, txt));
        }
        if let Some(sub) = p.as_struct() { find_in(sub, &ppath, needle, hits); }
        if let Some(els) = p.as_struct_array() {
            for (i, el) in els.iter().enumerate() { find_in(el, &format!("{}[{}]", ppath, i), needle, hits); }
        }
    }
}

/// Walk every level of the save and print paths whose name, struct type or
/// scalar value contains `needle` (case-insensitive).
fn cmd_find(path: &Path, needle: &str) {
    let needle = needle.to_lowercase();
    let data = std::fs::read(path).unwrap();
    let mut sf = SaveFile::parse(&data).unwrap();
    let mut hits = Vec::new();
    find_in(&sf.top, "top", &needle, &mut hits);
    for n in sf.model_names() {
        let Ok(m) = sf.model(&n) else { continue };
        if m.is_reference { continue; }
        let mpath = format!("PMD.{}", n);
        if n.to_lowercase().contains(&needle) || m.class_path.to_lowercase().contains(&needle) {
            hits.push(format!("{} : model {} ({} props)", mpath, m.class_path, m.props.props.len()));
        }
        find_in(&m.props, &mpath, &needle, &mut hits);
        if n == "SaveStateModel" {
            if let Some(els) = m.props.get("MechLoadoutWrappers").and_then(|p| p.as_struct_array()) {
                for (i, el) in els.iter().enumerate() {
                    if let Some(bd) = el.get("ByteData") {
                        if let Value::Array { body: ArrayBody::Raw(b), .. } = &bd.value {
                            if let Ok(arc) = Archive::parse(b) {
                                find_in(&arc.props, &format!("{}.MechLoadoutWrappers[{}].ByteData", mpath, i), &needle, &mut hits);
                            }
                        }
                    }
                }
            }
        }
    }
    for h in &hits { println!("{}", h); }
    println!("--- {} hits", hits.len());
}

fn repair_one(input: &Path, output: &Path) -> bool {
    let data = std::fs::read(input).unwrap();
    let (bytes, outcome) = mw5save::repair::repair(&data);
    let before = outcome.before.as_ref().map(|r| r.fatal()).unwrap_or(true);
    let after = outcome.after.as_ref().map(|r| r.fatal()).unwrap_or(true);
    println!("{} -> {}", input.display(), output.display());
    for s in &outcome.steps { println!("   fixed {:?} {} : {}", s.kind, s.path, s.action); }
    for u in &outcome.unrepairable { println!("   cannot: {}", u); }
    if outcome.changed() {
        std::fs::write(output, &bytes).unwrap();
    }
    // Success = was fatal, now not fatal, and the result round-trips.
    let rt = !after && roundtrip_one(&bytes).is_ok();
    println!("   before: {}  after: {}  roundtrip: {}", if before { "FATAL" } else { "ok" }, if after { "FATAL" } else { "ok" }, if rt { "ok" } else { "n/a" });
    !after
}

fn cmd_repair(input: &Path, output: &Path) {
    if !repair_one(input, output) { std::process::exit(1); }
}

/// Repair every .sav in a dir into outdir; report which came back clean.
fn cmd_repair_all(indir: &Path, outdir: &Path) {
    std::fs::create_dir_all(outdir).unwrap();
    let mut files = Vec::new();
    collect(indir, &mut files);
    let (mut fixed, mut not) = (0, 0);
    for f in &files {
        let out = outdir.join(f.file_name().unwrap());
        if repair_one(f, &out) { fixed += 1 } else { not += 1 }
    }
    println!("--- {} repaired to clean, {} not repairable", fixed, not);
}

fn print_report(label: &str, rep: &mw5save::diagnose::Report) {
    let verdict = if rep.fatal() { "FATAL" } else if rep.suspect() { "SUSPECT" } else { "CLEAN" };
    println!("{:<8} {}", verdict, label);
    for f in &rep.findings {
        println!("         {}", f);
    }
}

fn cmd_diagnose(path: &Path) {
    let mut files = Vec::new();
    collect(path, &mut files);
    let mut fatal = 0;
    for f in &files {
        let data = std::fs::read(f).unwrap();
        let rep = mw5save::diagnose::diagnose(&data);
        if rep.fatal() {
            fatal += 1;
        }
        print_report(&f.display().to_string(), &rep);
    }
    if files.len() > 1 {
        println!("--- {} files, {} fatal", files.len(), fatal);
    }
}

fn cmd_corrupt(kind: &str, input: &Path, output: &Path) {
    let data = std::fs::read(input).unwrap();
    match mw5save::corrupt::apply(kind, &data) {
        Ok(out) => {
            std::fs::write(output, &out).unwrap();
            println!("wrote {} ({} -> {} bytes)", output.display(), data.len(), out.len());
        }
        Err(e) => {
            eprintln!("corrupt {}: {}", kind, e);
            std::process::exit(1);
        }
    }
}

/// Generate every corruption kind from one donor and diagnose each. The
/// expected classification per kind is asserted so this doubles as a test.
fn cmd_fixtures(input: &Path, outdir: &Path) {
    use mw5save::diagnose::{diagnose, Kind};
    let data = std::fs::read(input).unwrap();
    std::fs::create_dir_all(outdir).unwrap();
    let base = diagnose(&data);
    print_report("donor", &base);
    if base.fatal() {
        eprintln!("donor is not clean; pick another");
        std::process::exit(1);
    }
    let expect = |kind: &str| -> Vec<Kind> {
        match kind {
            "truncate-mid" | "truncate-tail" => vec![Kind::Truncated],
            "size-inflate" | "size-shrink" => vec![Kind::SizeMismatch],
            "pmd-length" | "wrapper-length" => vec![Kind::ArchiveLength],
            "garbage-tail" => vec![Kind::TrailingBytes],
            _ => vec![],
        }
    };
    let mut failed = 0;
    for kind in mw5save::corrupt::KINDS {
        let out = match mw5save::corrupt::apply(kind, &data) {
            Ok(o) => o,
            Err(e) => {
                println!("SKIP     {} : {}", kind, e);
                continue;
            }
        };
        let p = outdir.join(format!("{}.sav", kind));
        std::fs::write(&p, &out).unwrap();
        let rep = diagnose(&out);
        let first = rep.findings.first().map(|f| f.kind.clone()).unwrap_or(Kind::Ok);
        let ok = expect(kind).contains(&first);
        if !ok {
            failed += 1;
        }
        print_report(&format!("{} {}", if ok { "pass" } else { "WRONG" }, p.display()), &rep);
    }
    println!("--- fixtures: {} wrong classifications", failed);
    if failed > 0 {
        std::process::exit(1);
    }
}

fn collect(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_dir() {
        let mut entries: Vec<_> = std::fs::read_dir(path).unwrap().flatten().map(|e| e.path()).collect();
        entries.sort();
        for e in entries {
            collect(&e, out);
        }
    } else if path.extension().map(|e| e == "sav").unwrap_or(false) {
        out.push(path.to_path_buf());
    }
}

fn first_diff(a: &[u8], b: &[u8]) -> String {
    if a.len() != b.len() {
        return format!("length {} vs {}", a.len(), b.len());
    }
    match a.iter().zip(b).position(|(x, y)| x != y) {
        Some(i) => format!("first diff at {:#x}", i),
        None => "identical".into(),
    }
}

/// Full-depth round trip: top -> PMD -> every model -> every MechLoadoutWrapper ByteData.
fn roundtrip_one(data: &[u8]) -> Result<String, String> {
    let mut sf = SaveFile::parse(data).map_err(|e| format!("top parse: {}", e))?;
    let names = sf.model_names();
    let mut wrappers = 0usize;
    let mut refs = 0usize;
    for n in &names {
        let m = sf.model(n).map_err(|e| format!("model {}: {}", n, e))?;
        if m.is_reference {
            refs += 1;
            continue;
        }
        if n == "SaveStateModel" {
            if let Some(w) = m.props.get_mut("MechLoadoutWrappers") {
                if let Value::Array { body: ArrayBody::Structs { elements, .. }, .. } = &mut w.value {
                    for el in elements.iter_mut() {
                        if let Some(bd) = el.get_mut("ByteData") {
                            if let Value::Array { body: ArrayBody::Raw(b), .. } = &mut bd.value {
                                let arc = Archive::parse(b).map_err(|e| format!("ByteData: {}", e))?;
                                *b = arc.to_bytes();
                                wrappers += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    let out = sf.to_bytes();
    if out == data {
        Ok(format!("OK  models={} refs={} wrappers={}", names.len() - refs, refs, wrappers))
    } else {
        Err(format!("MISMATCH {}", first_diff(data, &out)))
    }
}

fn cmd_roundtrip(path: &Path) {
    let mut files = Vec::new();
    collect(path, &mut files);
    let (mut ok, mut bad, mut skipped) = (0, 0, 0);
    for f in &files {
        let data = std::fs::read(f).unwrap();
        if !mw5save::save::looks_like_campaign_save(&data) {
            skipped += 1;
            println!("SKIP not a campaign save {} {}", data.len(), f.display());
            continue;
        }
        match roundtrip_one(&data) {
            Ok(msg) => {
                ok += 1;
                println!("{:<10} {} {}", msg, data.len(), f.display());
            }
            Err(msg) => {
                bad += 1;
                println!("FAIL {} {} {}", msg, data.len(), f.display());
            }
        }
    }
    println!("--- {} ok, {} failed, {} skipped, {} files", ok, bad, skipped, files.len());
    if bad > 0 {
        std::process::exit(1);
    }
}

fn cmd_models(path: &Path) {
    let data = std::fs::read(path).unwrap();
    let mut sf = SaveFile::parse(&data).unwrap();
    for n in sf.model_names() {
        match sf.model(&n) {
            Ok(m) if m.is_reference => println!("{:<32} {:<60} (object reference)", n, m.class_path),
            Ok(m) => println!("{:<32} {:<60} props={} footer={}", n, m.class_path, m.props.props.len(), m.footer.len()),
            Err(e) => println!("{:<32} ERROR {}", n, e),
        }
    }
}

fn describe(v: &Value) -> String {
    match v {
        Value::Struct { struct_name, body: StructBody::Tagged(pl), .. } => format!("Struct<{}> {} props", struct_name, pl.props.len()),
        Value::Struct { struct_name, body: StructBody::Raw(b), .. } => format!("Struct<{}> raw {} bytes", struct_name, b.len()),
        Value::Array { elem_type, body: ArrayBody::Structs { inner_struct, elements, .. }, .. } => format!("Array<{}:{}> x{}", elem_type, inner_struct, elements.len()),
        Value::Array { elem_type, body: ArrayBody::Raw(b), .. } => format!("Array<{}> raw {} bytes", elem_type, b.len()),
        Value::MapSet { sub_types, blob } => format!("MapSet<{}> {} bytes", sub_types.join(","), blob.len()),
        Value::Bool { value, .. } => format!("Bool {}", value),
        Value::Enum { enum_name, payload, .. } => {
            let mut r = mw5save::io::Reader::new(payload);
            format!("Enum<{}> {}", enum_name, r.fstring().unwrap_or_default())
        }
        Value::Scalar { payload, .. } => format!("{} bytes", payload.len()),
    }
}

fn dump_list(pl: &PropertyList, depth: usize, max_depth: usize) {
    for p in &pl.props {
        let extra = match p.ptype.as_str() {
            "IntProperty" => p.as_i32().map(|v| format!(" = {}", v)).unwrap_or_default(),
            "Int64Property" => p.as_i64().map(|v| format!(" = {}", v)).unwrap_or_default(),
            "FloatProperty" => p.as_f32().map(|v| format!(" = {}", v)).unwrap_or_default(),
            "StrProperty" | "NameProperty" | "ObjectProperty" => p.as_fstring().map(|v| format!(" = {}", v)).unwrap_or_default(),
            _ => String::new(),
        };
        println!("{}{} : {} [{}]{}", "  ".repeat(depth), p.name, p.ptype, describe(&p.value), extra);
        if depth < max_depth {
            if let Some(sub) = p.as_struct() {
                dump_list(sub, depth + 1, max_depth);
            }
            if let Some(els) = p.as_struct_array() {
                for (i, el) in els.iter().enumerate().take(3) {
                    println!("{}[{}]", "  ".repeat(depth + 1), i);
                    dump_list(el, depth + 2, max_depth);
                }
                if els.len() > 3 {
                    println!("{}... {} more", "  ".repeat(depth + 1), els.len() - 3);
                }
            }
        }
    }
}

fn cmd_dump(path: &Path, model: Option<&str>) {
    let data = std::fs::read(path).unwrap();
    let mut sf = SaveFile::parse(&data).unwrap();
    match model {
        None => dump_list(&sf.top, 0, 2),
        Some(m) => {
            let m = sf.model(m).unwrap();
            println!("class {}", m.class_path);
            dump_list(&m.props, 0, 4);
        }
    }
}
