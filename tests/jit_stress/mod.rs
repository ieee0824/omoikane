//! Reproducible Gate 4 stress in isolated child processes.

use std::{
    fs::{self, File},
    future::Future,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    task::{Context as TaskContext, Poll, Waker},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use boa_engine::{Context, JsError, JsValue, Script, Source};
use omoikane::{
    html::TreeBuilder,
    js::{JsRuntime, SandboxConfig},
};
use serde::Serialize;
use serde_json::{Value, json};

const SUPPORTED: bool = cfg!(all(
    target_arch = "x86_64",
    any(target_os = "linux", target_os = "macos")
));

#[derive(Debug, PartialEq, Eq, Serialize)]
struct OwnedError {
    runtime_limit: bool,
    message: String,
}

fn own_result(result: Result<JsValue, JsError>) -> Result<String, OwnedError> {
    result
        .map(|value| {
            value.as_string().map_or_else(
                || value.display().to_string(),
                |s| s.to_std_string_escaped(),
            )
        })
        .map_err(|error| OwnedError {
            runtime_limit: error.as_native().is_some_and(|e| e.is_runtime_limit()),
            message: error
                .as_native()
                .map_or_else(|| error.to_string(), |e| e.message().to_owned()),
        })
}

enum Engine {
    Standalone(Context),
    Embedded(JsRuntime),
}

struct Runner {
    engine: Engine,
    timeout: Duration,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct Counts {
    entries: u64,
    bailouts: u64,
    interrupts: u64,
    helpers: u64,
    exceptions: u64,
    shape_deopts: u64,
    type_deopts: u64,
    arithmetic_deopts: u64,
}

impl Runner {
    fn new(name: &str, timeout: Duration, iterations: u64) -> Self {
        let engine = if name == "boa-interpreter" {
            let mut context = Context::default();
            context.set_baseline_jit_enabled(false);
            context
                .runtime_limits_mut()
                .set_loop_iteration_limit(iterations);
            Engine::Standalone(context)
        } else {
            let document =
                TreeBuilder::parse("<!doctype html><html><head></head><body></body></html>")
                    .document();
            let mut runtime = JsRuntime::with_document_and_sandbox(
                document,
                SandboxConfig {
                    timeout,
                    max_loop_iterations: iterations,
                },
            )
            .unwrap();
            runtime.set_baseline_jit_enabled(name == "omoikane-jit");
            Engine::Embedded(runtime)
        };
        Self { engine, timeout }
    }

    fn eval(&mut self, source: &str, asynchronous: bool) -> Result<String, OwnedError> {
        let result = match &mut self.engine {
            Engine::Standalone(context) => {
                let mut scope = context.enter_runtime_deadline(Instant::now() + self.timeout);
                if asynchronous {
                    let script =
                        Script::parse(Source::from_bytes(source), None, &mut scope).unwrap();
                    poll_with_gc(script.evaluate_async_with_budget(&mut scope, 1024))
                } else {
                    scope.eval(Source::from_bytes(source))
                }
            }
            Engine::Embedded(runtime) => {
                if asynchronous {
                    poll_with_gc(runtime.eval_async(source))
                } else {
                    runtime.eval(source)
                }
            }
        };
        own_result(result)
    }

    fn snapshot(&self) -> String {
        match &self.engine {
            Engine::Standalone(context) => context.jit_debug_snapshot(),
            Engine::Embedded(runtime) => runtime.baseline_jit_debug_snapshot(),
        }
    }

    fn counts(&self) -> Counts {
        match &self.engine {
            Engine::Standalone(context) => {
                let a = context.arithmetic_jit_diagnostics();
                let e = context.jit_exception_diagnostics();
                Counts {
                    entries: a.compiled_entries,
                    bailouts: a.bailouts,
                    interrupts: a.interrupt_deopts,
                    helpers: e.generated_entries,
                    exceptions: e.exception_unwinds,
                    shape_deopts: a.shape_deopts,
                    type_deopts: a.type_deopts,
                    arithmetic_deopts: a.arithmetic_deopts,
                }
            }
            Engine::Embedded(runtime) => {
                let d = runtime.baseline_jit_diagnostics();
                Counts {
                    entries: d.compiled_entries,
                    bailouts: d.bailouts,
                    interrupts: d.interrupt_deopts,
                    helpers: d.runtime_helper_entries,
                    exceptions: d.exception_unwinds,
                    shape_deopts: d.shape_deopts,
                    type_deopts: d.type_deopts,
                    arithmetic_deopts: d.arithmetic_deopts,
                }
            }
        }
    }
}

fn poll_with_gc<F: Future>(future: F) -> F::Output {
    let mut future = Box::pin(future);
    let mut cx = TaskContext::from_waker(Waker::noop());
    let mut yields = 0;
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(result) => return result,
            Poll::Pending => {
                yields += 1;
                if yields <= 3 {
                    boa_gc::force_minor_collect();
                }
                if yields == 3 {
                    boa_gc::force_collect();
                }
                std::thread::yield_now();
            }
        }
    }
}

fn setup(seed: u64) -> String {
    let interval = 17 + seed % 19;
    let depth = 1 + seed % 4;
    format!(
        r#"
        globalThis.st={{payload:{{seed:'{seed}'}},keep:[],log:[]}};
        function hot(o,n,start){{let s=start;for(let i=0;i<n;i++){{s+=o.x;o.x+=1}}return s+o.x}}
        function arith(n,s){{for(let i=0;i<n;i++)s=s+i*3;return s}}
        function allocate(n,p,fail){{
            let last;
            try{{for(let i=0;i<n;i++){{last={{index:i,payload:p}};if(i%{interval}===0)st.keep.push(last)}}
                if(fail)throw last;return n;
            }}finally{{st.log.push('allocation')}}
        }}
        function nested(depth,n,fail){{
            try{{if(depth)return nested(depth-1,n,fail);return allocate(n,st.payload,fail)}}
            catch(e){{if(e.payload!==st.payload)throw 'lost payload';throw e}}
            finally{{st.log.push(depth)}}
        }}
        function spin(n){{let s=1;for(let i=0;i<n;i++)s=(s+i*3)%1000003;return s}}
        for(let i=0;i<40;i++)nested({depth},1,false);
        hot({{x:1}},200,0);arith(200,1);spin(200);
        st.keep=[];st.log=[];true
    "#
    )
}

fn save_phase(directory: &Path, runner: &Runner, phase: &str, source: &str) {
    fs::write(
        directory.join("phase.json"),
        serde_json::to_vec_pretty(&json!({"phase":phase,"source":source})).unwrap(),
    )
    .unwrap();
    fs::write(directory.join("jit-before.txt"), runner.snapshot()).unwrap();
}

fn evaluate_phase(
    directory: &Path,
    runner: &mut Runner,
    phase: &str,
    source: &str,
    asynchronous: bool,
) -> Result<String, OwnedError> {
    save_phase(directory, runner, phase, source);
    let result = runner.eval(source, asynchronous);
    fs::write(directory.join("jit-after.txt"), runner.snapshot()).unwrap();
    fs::write(
        directory.join(format!("{phase}-result.json")),
        serde_json::to_vec_pretty(&json!({"result":result,"counts":runner.counts()})).unwrap(),
    )
    .unwrap();
    result
}

fn run_seed(seed: u64, directory: &Path) -> Value {
    let depth = 1 + seed % 4;
    let setup = setup(seed);
    fs::write(directory.join("setup.js"), &setup).unwrap();
    let allocation = format!(
        "try{{nested({depth},40000,true)}}catch(e){{st.log.push(e.index,e.payload===st.payload)}}JSON.stringify([st.log,st.keep.length,st.keep.every(v=>v.payload===st.payload)])"
    );
    let guard = match seed % 3 {
        0 => "JSON.stringify(hot({pad:0,x:7},100,0))",
        1 => "JSON.stringify(arith('100',1))",
        _ => "JSON.stringify(hot({x:0},100,9007199254740980))",
    };
    let interrupt = "try{spin(1000000000000)}catch(e){st.log.push('caught limit')}finally{st.log.push('late finally')}";
    let recovery = "JSON.stringify([st.payload.seed,st.keep.every(v=>v.payload===st.payload),st.log.includes('caught limit'),st.log.includes('late finally'),6*7])";
    for (phase, source) in [
        ("allocation", allocation.as_str()),
        ("guard", guard),
        ("interrupt", interrupt),
        ("recovery", recovery),
    ] {
        fs::write(directory.join(format!("{phase}.js")), source).unwrap();
    }
    let mut reference = None;
    let mut rows = Vec::new();
    for name in ["boa-interpreter", "omoikane-interpreter", "omoikane-jit"] {
        let folder = directory.join(name);
        fs::create_dir(&folder).unwrap();
        let mut runner = Runner::new(name, Duration::from_secs(60), 50_000);
        let initial = runner.counts();
        runner.eval(&setup, false).unwrap();
        save_phase(&folder, &runner, "allocation", &allocation);
        if name == "omoikane-jit"
            && std::env::var_os("OMOIKANE_JIT_STRESS_EXPECTED_FAILURE").is_some()
        {
            panic!("intentional child failure after saving generated code");
        }
        boa_gc::force_collect();
        #[cfg(feature = "jit-stress")]
        boa_gc::reset_profile();
        let allocated =
            evaluate_phase(&folder, &mut runner, "allocation", &allocation, false).unwrap();
        #[cfg(feature = "jit-stress")]
        let collections = {
            let p = boa_gc::profile();
            assert!(
                p.minor.collections + p.major.collections > 0,
                "allocation phase must collect automatically"
            );
            json!({"minor":p.minor.collections,"major":p.major.collections})
        };
        #[cfg(not(feature = "jit-stress"))]
        let collections = Value::Null;
        let allocated_json: Value = serde_json::from_str(&allocated).unwrap();
        assert_eq!(allocated_json[2], true);
        assert!(allocated_json[1].as_u64().unwrap() > 0);
        boa_gc::force_minor_collect();
        boa_gc::force_collect();
        let before_guard = runner.counts();
        let guarded = evaluate_phase(&folder, &mut runner, "guard", guard, seed % 2 == 0).unwrap();
        let after_guard = runner.counts();
        let error = evaluate_phase(&folder, &mut runner, "interrupt", interrupt, seed % 2 != 0)
            .unwrap_err();
        assert!(error.runtime_limit);
        assert_ne!(error.message, boa_engine::vm::WALL_CLOCK_TIMEOUT_MESSAGE);
        boa_gc::force_collect();
        let recovered = evaluate_phase(&folder, &mut runner, "recovery", recovery, false).unwrap();
        let recovered_json: Value = serde_json::from_str(&recovered).unwrap();
        assert_eq!(
            recovered_json,
            json!([seed.to_string(), true, false, false, 42])
        );
        let counts = runner.counts();
        if name == "omoikane-jit" {
            assert!(counts.entries > initial.entries);
            assert!(counts.helpers > initial.helpers);
            assert!(counts.exceptions > initial.exceptions);
            assert!(after_guard.bailouts > before_guard.bailouts);
            let (before, after) = match seed % 3 {
                0 => (before_guard.shape_deopts, after_guard.shape_deopts),
                1 => (before_guard.type_deopts, after_guard.type_deopts),
                _ => (
                    before_guard.arithmetic_deopts,
                    after_guard.arithmetic_deopts,
                ),
            };
            assert!(
                after > before,
                "seed {seed}: selected guard must deoptimize"
            );
            assert!(counts.interrupts > initial.interrupts);
        }
        fs::write(folder.join("jit-after.txt"), runner.snapshot()).unwrap();
        let outcomes =
            json!({"allocation":allocated,"guard":guarded,"interrupt":error,"recovery":recovered});
        if let Some(expected) = &reference {
            assert_eq!(&outcomes, expected, "seed={seed} backend={name}");
        } else {
            reference = Some(outcomes.clone());
        }
        drop(runner);
        boa_gc::force_collect();

        // The wall-clock variant starts from a fresh runtime with the same
        // seeded functions, and collects between asynchronous slices.
        let mut timed = Runner::new(name, Duration::from_millis(200), u64::MAX);
        timed.eval(&setup, false).unwrap();
        let before = timed.counts();
        boa_gc::force_collect();
        let source = "try{try{throw st.payload}catch(e){st.log.push(e===st.payload)}spin(1000000000000)}catch(e){st.log.push('caught timeout')}finally{st.log.push('late timeout')}";
        let error = evaluate_phase(&folder, &mut timed, "wall-clock", source, true).unwrap_err();
        assert!(error.runtime_limit);
        assert_eq!(error.message, boa_engine::vm::WALL_CLOCK_TIMEOUT_MESSAGE);
        assert_eq!(timed.eval("JSON.stringify([st.log.includes(true),st.log.includes('caught timeout'),st.log.includes('late timeout'),21*2])",false).unwrap(),"[true,false,false,42]");
        if name == "omoikane-jit" {
            assert!(timed.counts().entries > before.entries);
        }
        rows.push(json!({"backend":name,"outcomes":outcomes,"counts":counts,"guard_before":before_guard,"guard_after":after_guard,"automatic_gc":collections,"wall_clock":error}));
        drop(timed);
        boa_gc::force_collect();
    }
    json!({"seed":seed,"guard":seed%3,"async_variant":seed%2,"rows":rows})
}

#[test]
fn seed_replay() {
    let Ok(seed) = std::env::var("OMOIKANE_JIT_STRESS_SEED") else {
        return;
    };
    let seed = seed.parse::<u64>().expect("decimal seed");
    assert!(SUPPORTED, "native stress requires a supported JIT target");
    let directory =
        PathBuf::from(std::env::var_os("OMOIKANE_JIT_STRESS_DIR").expect("artifact directory"));
    let report = run_seed(seed, &directory);
    fs::write(
        directory.join("result.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
}

fn run_directory(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = PathBuf::from(".artifacts/js-benchmark/jit-stress")
        .join(format!("{label}-{stamp}-{}", std::process::id()));
    fs::create_dir_all(&path).unwrap();
    path
}

fn run_child(seed: u64, folder: &Path, inject_failure: bool) -> bool {
    fs::create_dir_all(folder).unwrap();
    fs::write(
        folder.join("seed.json"),
        serde_json::to_vec_pretty(&json!({"seed":seed,"injected_failure":inject_failure})).unwrap(),
    )
    .unwrap();
    let executable = std::env::current_exe().unwrap();
    let mut command = if let Ok(runner) = std::env::var("OMOIKANE_JIT_STRESS_RUNNER") {
        let args: Vec<String> =
            serde_json::from_str(&runner).expect("runner must be a JSON argument array");
        assert!(!args.is_empty());
        let mut command = Command::new(&args[0]);
        command.args(&args[1..]);
        command.arg(&executable);
        command
    } else {
        Command::new(&executable)
    };
    command
        .args([
            "--exact",
            "stress::seed_replay",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("OMOIKANE_JIT_STRESS_SEED", seed.to_string())
        .env("OMOIKANE_JIT_STRESS_DIR", folder)
        .env_remove("OMOIKANE_JIT_STRESS_EXPECTED_FAILURE")
        .stdout(Stdio::from(
            File::create(folder.join("stdout.log")).unwrap(),
        ))
        .stderr(Stdio::from(
            File::create(folder.join("stderr.log")).unwrap(),
        ));
    if inject_failure {
        command.env("OMOIKANE_JIT_STRESS_EXPECTED_FAILURE", "1");
    }
    let mut child = command
        .spawn()
        .expect("launch seed replay (configure JSON runner for emulation)");
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            fs::write(
                folder.join("exit.json"),
                serde_json::to_vec_pretty(
                    &json!({"success":status.success(),"status":status.to_string()}),
                )
                .unwrap(),
            )
            .unwrap();
            return status.success();
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            let status = child.wait().unwrap();
            fs::write(
                folder.join("exit.json"),
                serde_json::to_vec_pretty(
                    &json!({"success":false,"watchdog":true,"status":status.to_string()}),
                )
                .unwrap(),
            )
            .unwrap();
            return false;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn reproducible_stress_matrix() {
    if !SUPPORTED {
        eprintln!("JIT stress: unsupported target; no native gate result");
        return;
    }
    let count = std::env::var("OMOIKANE_JIT_STRESS_SEEDS")
        .map_or(8, |s| s.parse::<u64>().expect("seed count"));
    assert!(count > 0 && count <= 10_000);
    let first = std::env::var("OMOIKANE_JIT_STRESS_FIRST_SEED")
        .map_or(0, |s| s.parse::<u64>().expect("first seed"));
    let minimum_seconds = std::env::var("OMOIKANE_JIT_STRESS_MIN_SECONDS")
        .map_or(0, |s| s.parse::<u64>().expect("minimum run duration"));
    assert!(minimum_seconds <= 3600);
    let minimum_duration = Duration::from_secs(minimum_seconds);
    let root = run_directory("matrix");
    let start = Instant::now();
    let mut elapsed = Duration::ZERO;
    let mut completed = Vec::new();
    while (completed.len() as u64) < count || elapsed < minimum_duration {
        let offset = completed.len() as u64;
        assert!(
            offset < 10_000,
            "stress seed limit reached before duration target"
        );
        let seed = first.checked_add(offset).unwrap();
        let folder = root.join(format!("seed-{seed}"));
        let success = run_child(seed, &folder, false);
        completed.push(json!({"seed":seed,"success":success}));
        elapsed = start.elapsed();
        let report = json!({"target":format!("{}-{}",std::env::consts::ARCH,std::env::consts::OS),
            "emulated":std::env::var_os("OMOIKANE_JIT_STRESS_RUNNER").is_some(),"gc_profile":cfg!(feature="jit-stress"),
            "minimum_seconds":minimum_seconds,"elapsed_seconds":elapsed.as_secs_f64(),"seeds":completed});
        fs::write(
            root.join("summary.json"),
            serde_json::to_vec_pretty(&report).unwrap(),
        )
        .unwrap();
        assert!(
            success,
            "seed {seed} failed; artifacts: {}",
            folder.display()
        );
        // Retain full snapshots for the first successful seed and every failed
        // seed. Remove only bulky files this run created after verified success.
        if offset != 0 {
            for backend in ["boa-interpreter", "omoikane-interpreter", "omoikane-jit"] {
                for file in ["jit-before.txt", "jit-after.txt"] {
                    fs::remove_file(folder.join(backend).join(file)).unwrap();
                }
            }
        }
    }
    println!(
        "JIT stress: {} seeds passed in {:.1}s; {}",
        completed.len(),
        start.elapsed().as_secs_f64(),
        root.display()
    );
}

#[test]
fn child_failure_preserves_seed_source_code_and_stack_maps() {
    if !SUPPORTED {
        return;
    }
    let root = run_directory("artifact-self-test");
    let folder = root.join("seed-0");
    assert!(!run_child(0, &folder, true));
    assert!(folder.join("seed.json").is_file());
    assert!(folder.join("setup.js").is_file());
    let snapshot = fs::read_to_string(folder.join("omoikane-jit/jit-before.txt")).unwrap();
    assert!(snapshot.contains("tier=arithmetic"));
    assert!(snapshot.contains("StackMap"));
    assert!(snapshot.contains("DeoptRecipe"));
    assert!(snapshot.contains("00000000:"));
    let output = fs::read_to_string(folder.join("stderr.log")).unwrap();
    assert!(
        output.contains("intentional child failure after saving generated code"),
        "{output}"
    );
}
