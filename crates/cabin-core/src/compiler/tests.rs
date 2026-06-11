use super::report::{ar_capabilities_as_json, cxx_capabilities_as_json};
use super::*;

#[test]
fn parses_clang_first_line() {
    let id = parse_cxx_version_output(
        "clang version 17.0.6\nTarget: x86_64-unknown-linux-gnu\nThread model: posix\n",
    );
    assert_eq!(id.kind, CompilerKind::Clang);
    let v = id.version.expect("version parsed");
    assert_eq!(v.major, 17);
    assert_eq!(v.minor, Some(0));
    assert_eq!(v.patch, Some(6));
    assert_eq!(id.target.as_deref(), Some("x86_64-unknown-linux-gnu"));
}

#[test]
fn parses_apple_clang() {
    let id = parse_cxx_version_output(
        "Apple clang version 14.0.3 (clang-1403.0.22.14.1)\nTarget: arm64-apple-darwin22.5.0\nThread model: posix\n",
    );
    assert_eq!(id.kind, CompilerKind::AppleClang);
    let v = id.version.unwrap();
    assert_eq!((v.major, v.minor, v.patch), (14, Some(0), Some(3)));
}

#[test]
fn parses_gcc_with_distro_prefix() {
    let id = parse_cxx_version_output(
        "g++ (Ubuntu 11.4.0-1ubuntu1) 11.4.0\nCopyright (C) 2021 Free Software Foundation, Inc.\n",
    );
    assert_eq!(id.kind, CompilerKind::Gcc);
    let v = id.version.unwrap();
    assert_eq!((v.major, v.minor, v.patch), (11, Some(4), Some(0)));
}

#[test]
fn parses_msvc_first_line() {
    let id = parse_cxx_version_output(
        "Microsoft (R) C/C++ Optimizing Compiler Version 19.39.33523 for x64\n",
    );
    assert_eq!(id.kind, CompilerKind::Msvc);
    let v = id.version.unwrap();
    assert_eq!(v.major, 19);
}

#[test]
fn unknown_when_unrecognized() {
    let id = parse_cxx_version_output("My funky compiler 0.0\n");
    assert_eq!(id.kind, CompilerKind::Unknown);
    assert!(id.version.is_none());
}

#[test]
fn empty_output_is_unknown() {
    let id = parse_cxx_version_output("");
    assert_eq!(id.kind, CompilerKind::Unknown);
    assert!(id.raw_version_line.is_empty());
}

#[test]
fn parses_gnu_ar() {
    let id = parse_ar_version_output(
        "GNU ar (GNU Binutils for Debian) 2.40\nCopyright (C) 2023 Free Software Foundation, Inc.\n",
    );
    assert_eq!(id.kind, ArchiverKind::Ar);
    let v = id.version.unwrap();
    assert_eq!(v.major, 2);
}

#[test]
fn parses_llvm_ar_version() {
    let id = parse_ar_version_output(
        "LLVM (http://llvm.org/):\n  LLVM version 17.0.6\n  Optimized build.\n",
    );
    assert_eq!(id.kind, ArchiverKind::LlvmAr);
    let v = id.version.unwrap();
    assert_eq!(v.major, 17);
}

#[test]
fn detects_lib_exe_as_unsupported() {
    let id = parse_ar_version_output(
        "Microsoft (R) Library Manager Version 14.39.33523.0\nCopyright (C) Microsoft Corporation.\n",
    );
    assert_eq!(id.kind, ArchiverKind::Lib);
}

#[test]
fn unknown_archiver_classification() {
    let id = parse_ar_version_output("just-some-archiver 0.1\n");
    assert_eq!(id.kind, ArchiverKind::Unknown);
    assert!(id.version.is_none());
}

#[test]
fn clang_capabilities_include_gcc_style_and_cxx17() {
    let id = CompilerIdentity {
        kind: CompilerKind::Clang,
        version: CompilerVersion::parse("17.0.6"),
        target: None,
        raw_version_line: "clang version 17.0.6".into(),
    };
    let caps = derive_cxx_capabilities(&id);
    assert!(caps.gcc_style_flags.supported);
    assert!(caps.depfile_mmd_mf.supported);
    assert!(caps.std_flag.supported);
    assert!(caps.cxx_standard_17.supported);
}

#[test]
fn gcc_pre_5_does_not_claim_cxx17() {
    let id = CompilerIdentity {
        kind: CompilerKind::Gcc,
        version: CompilerVersion::parse("4.8.5"),
        target: None,
        raw_version_line: "g++ 4.8.5".into(),
    };
    let caps = derive_cxx_capabilities(&id);
    assert!(caps.gcc_style_flags.supported);
    assert!(!caps.cxx_standard_17.supported);
}

#[test]
fn msvc_capabilities_reject_gcc_style() {
    let id = CompilerIdentity {
        kind: CompilerKind::Msvc,
        version: CompilerVersion::parse("19.39.0"),
        target: None,
        raw_version_line: "Microsoft Optimizing Compiler".into(),
    };
    let caps = derive_cxx_capabilities(&id);
    assert!(!caps.gcc_style_flags.supported);
    assert_eq!(caps.gcc_style_flags.source, CapabilitySource::Unsupported);
    assert!(caps.msvc_style_flags.supported);
}

#[test]
fn unknown_compiler_capabilities_are_conservative() {
    let id = CompilerIdentity::unknown("strange compiler");
    let caps = derive_cxx_capabilities(&id);
    assert!(!caps.gcc_style_flags.supported);
    assert_eq!(
        caps.gcc_style_flags.source,
        CapabilitySource::AssumedDefault
    );
    assert!(!caps.depfile_mmd_mf.supported);
}

#[test]
fn ar_capabilities_recognize_gnu_ar() {
    let id = ArchiverIdentity {
        kind: ArchiverKind::Ar,
        version: CompilerVersion::parse("2.40"),
        raw_version_line: "GNU ar".into(),
    };
    let caps = derive_ar_capabilities(&id);
    assert!(caps.ar_crs.supported);
    assert!(caps.static_library_output.supported);
}

#[test]
fn msvc_lib_archives_without_ar_crs() {
    // `lib.exe` does not accept GNU `crs` mode flags, but it
    // does produce a static library (`lib /OUT:`), so metadata
    // must report `static_library_output` as supported.
    let id = ArchiverIdentity {
        kind: ArchiverKind::Lib,
        version: None,
        raw_version_line: "Microsoft Library Manager".into(),
    };
    let caps = derive_ar_capabilities(&id);
    assert!(!caps.ar_crs.supported);
    assert_eq!(caps.ar_crs.source, CapabilitySource::Unsupported);
    assert!(caps.static_library_output.supported);
}

#[test]
fn validate_accepts_msvc_cxx() {
    // MSVC drives the `cl.exe` backend; detection reports
    // `msvc_style_flags`, so validation must accept it.
    let id = CompilerIdentity {
        kind: CompilerKind::Msvc,
        version: None,
        target: None,
        raw_version_line: "MSVC".into(),
    };
    let caps = derive_cxx_capabilities(&id);
    assert!(caps.msvc_style_flags.supported);
    assert!(validate_cxx_for_backend("cl.exe", &id, &caps).is_ok());
}

fn msvc_identity(version: &str) -> CompilerIdentity {
    CompilerIdentity {
        kind: CompilerKind::Msvc,
        version: CompilerVersion::parse(version),
        target: None,
        raw_version_line: format!("Microsoft Optimizing Compiler {version}"),
    }
}

#[test]
fn msvc_std_capabilities_are_version_gated() {
    // `/std:c++17` needs cl 19.11 (VS2017 15.3); `/std:c11` needs
    // cl 19.28 (VS2019 16.8).
    let modern = derive_cxx_capabilities(&msvc_identity("19.39.33523"));
    assert!(modern.cxx_standard_17.supported);
    assert_eq!(modern.cxx_standard_17.source, CapabilitySource::Version);
    assert!(modern.c_standard_11.supported);
    assert_eq!(modern.c_standard_11.source, CapabilitySource::Version);

    // cl 19.20 (VS2019 16.0) takes /std:c++17 but predates /std:c11.
    let mid = derive_cxx_capabilities(&msvc_identity("19.20.0"));
    assert!(mid.cxx_standard_17.supported);
    assert!(!mid.c_standard_11.supported);
    assert_eq!(mid.c_standard_11.source, CapabilitySource::Version);

    // cl 19.00 (VS2015) predates both switches.
    let old = derive_cxx_capabilities(&msvc_identity("19.00.24210"));
    assert!(!old.cxx_standard_17.supported);
    assert!(!old.c_standard_11.supported);
}

#[test]
fn msvc_unparsed_version_assumes_modern_support() {
    // A real `cl` always reports a version; a parse miss
    // (`version: None`) must NOT reject an otherwise-modern
    // compiler, so the gate defaults to supported/assumed-default.
    let caps = derive_cxx_capabilities(&CompilerIdentity {
        kind: CompilerKind::Msvc,
        version: None,
        target: None,
        raw_version_line: "Microsoft Optimizing Compiler".into(),
    });
    assert!(caps.cxx_standard_17.supported);
    assert_eq!(
        caps.cxx_standard_17.source,
        CapabilitySource::AssumedDefault
    );
    assert!(caps.c_standard_11.supported);
    assert_eq!(caps.c_standard_11.source, CapabilitySource::AssumedDefault);
}

#[test]
fn gnu_c_standard_11_is_unconditional() {
    // `-std=c11` has been available far longer than `-std=c++17`,
    // so every recognized GCC/Clang reports it regardless of major.
    for id in [
        CompilerIdentity {
            kind: CompilerKind::Gcc,
            version: CompilerVersion::parse("4.8.5"),
            target: None,
            raw_version_line: "g++ 4.8.5".into(),
        },
        CompilerIdentity {
            kind: CompilerKind::Clang,
            version: CompilerVersion::parse("3.4"),
            target: None,
            raw_version_line: "clang version 3.4".into(),
        },
    ] {
        assert!(derive_cxx_capabilities(&id).c_standard_11.supported);
    }
}

#[test]
fn validate_rejects_msvc_too_old_for_std_flags() {
    let old = msvc_identity("19.00.24210");
    let caps = derive_cxx_capabilities(&old);
    // C++ build: rejected for lacking /std:c++17.
    assert!(matches!(
        validate_cxx_for_backend("cl.exe", &old, &caps),
        Err(ToolDetectionError::CxxLacksStdCxx17 { .. })
    ));
    // C build: rejected for lacking /std:c11.
    assert!(matches!(
        validate_cc_for_backend("cl.exe", &old, &caps),
        Err(ToolDetectionError::CLacksStdC11 { .. })
    ));
}

#[test]
fn clang_cl_has_msvc_dialect_with_clang_diagnostics() {
    // `clang-cl` reports a clang version, but speaks the MSVC
    // dialect: MSVC-style flags yes, GCC-style/depfile no, while
    // keeping Clang's color/json/response-file capabilities and
    // version-independent C++17/C11 support.
    let id = CompilerIdentity {
        kind: CompilerKind::ClangCl,
        version: CompilerVersion::parse("17.0.6"),
        target: None,
        raw_version_line: "clang version 17.0.6".into(),
    };
    assert!(id.kind.speaks_msvc_dialect());
    assert!(id.kind.is_clang_like());
    assert!(!id.kind.supports_gcc_style_command_line());

    let caps = derive_cxx_capabilities(&id);
    assert!(caps.msvc_style_flags.supported);
    assert!(!caps.gcc_style_flags.supported);
    assert!(!caps.depfile_mmd_mf.supported);
    assert!(caps.cxx_standard_17.supported);
    assert!(caps.c_standard_11.supported);

    // Validates against both the C++ and C MSVC backends.
    assert!(validate_cxx_for_backend("clang-cl", &id, &caps).is_ok());
    assert!(validate_cc_for_backend("clang-cl", &id, &caps).is_ok());
}

#[test]
fn validate_accepts_modern_and_unversioned_msvc_c() {
    for id in [
        msvc_identity("19.39.33523"),
        CompilerIdentity {
            kind: CompilerKind::Msvc,
            version: None,
            target: None,
            raw_version_line: "MSVC".into(),
        },
    ] {
        let caps = derive_cxx_capabilities(&id);
        assert!(validate_cc_for_backend("cl.exe", &id, &caps).is_ok());
    }
}

#[test]
fn validate_rejects_unknown_cxx() {
    let id = CompilerIdentity::unknown("???");
    let caps = derive_cxx_capabilities(&id);
    let err = validate_cxx_for_backend("custom-cxx", &id, &caps).unwrap_err();
    assert!(matches!(
        err,
        ToolDetectionError::UnknownCxxRequiresGccStyle { .. }
    ));
}

#[test]
fn validate_accepts_clang() {
    let id = CompilerIdentity {
        kind: CompilerKind::Clang,
        version: CompilerVersion::parse("17.0.6"),
        target: None,
        raw_version_line: "clang version 17.0.6".into(),
    };
    let caps = derive_cxx_capabilities(&id);
    assert!(validate_cxx_for_backend("clang++", &id, &caps).is_ok());
}

#[test]
fn validate_rejects_gcc_too_old_for_cxx17() {
    let id = CompilerIdentity {
        kind: CompilerKind::Gcc,
        version: CompilerVersion::parse("4.8.5"),
        target: None,
        raw_version_line: "g++ 4.8".into(),
    };
    let caps = derive_cxx_capabilities(&id);
    let err = validate_cxx_for_backend("g++", &id, &caps).unwrap_err();
    assert!(matches!(err, ToolDetectionError::CxxLacksStdCxx17 { .. }));
}

#[test]
fn validate_cc_accepts_pure_c_clang_without_cxx17_capability() {
    // The C-side validator must accept a compiler that
    // would *not* satisfy the C++ contract (no
    // `cxx_standard_17`). A bare `cc` driver on a system
    // that ships only C headers is a legitimate case; only
    // GCC-style flags + depfile are required for the C
    // backend.
    let id = CompilerIdentity {
        kind: CompilerKind::Clang,
        version: CompilerVersion::parse("17.0.6"),
        target: None,
        raw_version_line: "clang version 17.0.6".into(),
    };
    let mut caps = derive_cxx_capabilities(&id);
    // Force `cxx_standard_17` off so we can be certain the
    // C validator does not gate on it.
    caps.cxx_standard_17 = Capability {
        supported: false,
        source: CapabilitySource::Unsupported,
    };
    assert!(validate_cc_for_backend("cc", &id, &caps).is_ok());
    // Sanity: the equivalent CXX validation would now reject
    // the same compiler. Asserting both directions
    // documents the design constraint that C/C++
    // capability gating differ.
    assert!(matches!(
        validate_cxx_for_backend("cc", &id, &caps).unwrap_err(),
        ToolDetectionError::CxxLacksStdCxx17 { .. }
    ));
}

#[test]
fn validate_cc_accepts_msvc() {
    let id = CompilerIdentity {
        kind: CompilerKind::Msvc,
        version: None,
        target: None,
        raw_version_line: "MSVC".into(),
    };
    let caps = derive_cxx_capabilities(&id);
    assert!(validate_cc_for_backend("cl.exe", &id, &caps).is_ok());
}

#[test]
fn validate_cc_rejects_unknown_compiler_without_gcc_style() {
    // Unknown identity + missing `gcc_style_flags` capability
    // is the unrecoverable case: the planner cannot tell
    // whether the compiler accepts `-c -o` etc.
    let id = CompilerIdentity::unknown("???");
    let caps = derive_cxx_capabilities(&id);
    let err = validate_cc_for_backend("custom-cc", &id, &caps).unwrap_err();
    assert!(matches!(
        err,
        ToolDetectionError::UnknownCRequiresGccStyle { .. }
    ));
}

#[test]
fn validate_cc_rejects_gcc_without_depfile_support() {
    // GCC identity but without `-MMD -MF` support — Cabin
    // emits a depfile flag for every compile so the C
    // contract requires it, even though `cxx_standard_17`
    // is not relevant.
    let id = CompilerIdentity {
        kind: CompilerKind::Gcc,
        version: CompilerVersion::parse("9.4.0"),
        target: None,
        raw_version_line: "gcc 9.4".into(),
    };
    let mut caps = derive_cxx_capabilities(&id);
    caps.depfile_mmd_mf = Capability {
        supported: false,
        source: CapabilitySource::Unsupported,
    };
    let err = validate_cc_for_backend("cc", &id, &caps).unwrap_err();
    assert!(matches!(err, ToolDetectionError::CLacksDepfile { .. }));
}

#[test]
fn validate_accepts_msvc_archiver() {
    // `lib.exe` is the MSVC static-library archiver.
    let id = ArchiverIdentity {
        kind: ArchiverKind::Lib,
        version: None,
        raw_version_line: "Microsoft Library Manager".into(),
    };
    let caps = derive_ar_capabilities(&id);
    assert!(validate_ar_for_backend("lib.exe", &id, &caps).is_ok());
}

#[test]
fn version_display_truncates_unset_components() {
    let v = CompilerVersion::parse("11").unwrap();
    assert_eq!(v.to_display_string(), "11");
    let v = CompilerVersion::parse("11.4").unwrap();
    assert_eq!(v.to_display_string(), "11.4");
    let v = CompilerVersion::parse("11.4.0").unwrap();
    assert_eq!(v.to_display_string(), "11.4.0");
}

// --------------------------------------------------------------
// Golden / fixture tests.
//
// These pin the JSON shape that downstream tooling
// (`cabin metadata`, IDE integrations) reads out of a
// `ToolchainDetectionReport`. Any accidental change to the
// field names or serialization order here is user-visible
// and should be deliberate.
// --------------------------------------------------------------

fn pretty(value: &serde_json::Value) -> String {
    serde_json::to_string_pretty(value).unwrap()
}

fn cxx_identity_and_capabilities_json(version_output: &str) -> String {
    let id = parse_cxx_version_output(version_output);
    let caps = derive_cxx_capabilities(&id);
    pretty(&serde_json::json!({
        "identity": id.as_json(),
        "capabilities": cxx_capabilities_as_json(&caps),
    }))
}

fn ar_identity_and_capabilities_json(version_output: &str) -> String {
    let id = parse_ar_version_output(version_output);
    let caps = derive_ar_capabilities(&id);
    pretty(&serde_json::json!({
        "identity": id.as_json(),
        "capabilities": ar_capabilities_as_json(&caps),
    }))
}

#[test]
fn snapshot_clang_identity_and_capabilities() {
    let actual = cxx_identity_and_capabilities_json(
        "clang version 17.0.6\nTarget: x86_64-unknown-linux-gnu\nThread model: posix\n",
    );
    let expected = r#"{
  "identity": {
    "kind": "clang",
    "version": "17.0.6",
    "target": "x86_64-unknown-linux-gnu",
    "raw_version_line": "clang version 17.0.6"
  },
  "capabilities": {
    "c_standard_11": {
      "supported": true,
      "source": "version"
    },
    "cxx_standard_17": {
      "supported": true,
      "source": "version"
    },
    "depfile_mmd_mf": {
      "supported": true,
      "source": "version"
    },
    "gcc_style_flags": {
      "supported": true,
      "source": "version"
    },
    "msvc_style_flags": {
      "supported": false,
      "source": "assumed-default"
    },
    "std_flag": {
      "supported": true,
      "source": "version"
    }
  }
}"#;
    assert_eq!(actual, expected);
}

#[test]
fn snapshot_apple_clang_identity_and_capabilities() {
    let actual = cxx_identity_and_capabilities_json(
        "Apple clang version 14.0.3 (clang-1403.0.22.14.1)\nTarget: arm64-apple-darwin22.5.0\nThread model: posix\n",
    );
    let expected = r#"{
  "identity": {
    "kind": "apple-clang",
    "version": "14.0.3",
    "target": "arm64-apple-darwin22.5.0",
    "raw_version_line": "Apple clang version 14.0.3 (clang-1403.0.22.14.1)"
  },
  "capabilities": {
    "c_standard_11": {
      "supported": true,
      "source": "version"
    },
    "cxx_standard_17": {
      "supported": true,
      "source": "version"
    },
    "depfile_mmd_mf": {
      "supported": true,
      "source": "version"
    },
    "gcc_style_flags": {
      "supported": true,
      "source": "version"
    },
    "msvc_style_flags": {
      "supported": false,
      "source": "assumed-default"
    },
    "std_flag": {
      "supported": true,
      "source": "version"
    }
  }
}"#;
    assert_eq!(actual, expected);
}

#[test]
fn snapshot_gcc_identity_and_capabilities() {
    let actual = cxx_identity_and_capabilities_json(
        "g++ (Ubuntu 11.4.0-1ubuntu1) 11.4.0\nCopyright (C) 2021 Free Software Foundation, Inc.\n",
    );
    let expected = r#"{
  "identity": {
    "kind": "gcc",
    "version": "11.4.0",
    "raw_version_line": "g++ (Ubuntu 11.4.0-1ubuntu1) 11.4.0"
  },
  "capabilities": {
    "c_standard_11": {
      "supported": true,
      "source": "version"
    },
    "cxx_standard_17": {
      "supported": true,
      "source": "version"
    },
    "depfile_mmd_mf": {
      "supported": true,
      "source": "version"
    },
    "gcc_style_flags": {
      "supported": true,
      "source": "version"
    },
    "msvc_style_flags": {
      "supported": false,
      "source": "assumed-default"
    },
    "std_flag": {
      "supported": true,
      "source": "version"
    }
  }
}"#;
    assert_eq!(actual, expected);
}

#[test]
fn snapshot_msvc_identity_and_capabilities() {
    let actual = cxx_identity_and_capabilities_json(
        "Microsoft (R) C/C++ Optimizing Compiler Version 19.39.33523 for x64\n",
    );
    // A modern `cl` (19.39 == VS2022 17.9) accepts the
    // `/std:c++17` and `/std:c11` switches Cabin emits, so both
    // standard capabilities are version-supported; the GCC-style
    // and depfile capabilities stay unsupported because MSVC
    // drives its own dialect.
    let expected = r#"{
  "identity": {
    "kind": "msvc",
    "version": "19.39.33523",
    "raw_version_line": "Microsoft (R) C/C++ Optimizing Compiler Version 19.39.33523 for x64"
  },
  "capabilities": {
    "c_standard_11": {
      "supported": true,
      "source": "version"
    },
    "cxx_standard_17": {
      "supported": true,
      "source": "version"
    },
    "depfile_mmd_mf": {
      "supported": false,
      "source": "unsupported"
    },
    "gcc_style_flags": {
      "supported": false,
      "source": "unsupported"
    },
    "msvc_style_flags": {
      "supported": true,
      "source": "version"
    },
    "std_flag": {
      "supported": false,
      "source": "unsupported"
    }
  }
}"#;
    assert_eq!(actual, expected);
}

#[test]
fn snapshot_unknown_compiler_capabilities_are_conservative() {
    let actual = cxx_identity_and_capabilities_json("My funky compiler 0.0\n");
    let expected = r#"{
  "identity": {
    "kind": "unknown",
    "raw_version_line": "My funky compiler 0.0"
  },
  "capabilities": {
    "c_standard_11": {
      "supported": false,
      "source": "assumed-default"
    },
    "cxx_standard_17": {
      "supported": false,
      "source": "assumed-default"
    },
    "depfile_mmd_mf": {
      "supported": false,
      "source": "assumed-default"
    },
    "gcc_style_flags": {
      "supported": false,
      "source": "assumed-default"
    },
    "msvc_style_flags": {
      "supported": false,
      "source": "assumed-default"
    },
    "std_flag": {
      "supported": false,
      "source": "assumed-default"
    }
  }
}"#;
    assert_eq!(actual, expected);
}

#[test]
fn snapshot_gnu_ar_identity_and_capabilities() {
    let actual = ar_identity_and_capabilities_json(
        "GNU ar (GNU Binutils for Debian) 2.40\nCopyright (C) 2023 Free Software Foundation, Inc.\n",
    );
    let expected = r#"{
  "identity": {
    "kind": "ar",
    "version": "2.40",
    "raw_version_line": "GNU ar (GNU Binutils for Debian) 2.40"
  },
  "capabilities": {
    "ar_crs": {
      "supported": true,
      "source": "version"
    },
    "static_library_output": {
      "supported": true,
      "source": "version"
    }
  }
}"#;
    assert_eq!(actual, expected);
}

#[test]
fn snapshot_msvc_lib_archiver_produces_static_library_without_ar_crs() {
    let actual = ar_identity_and_capabilities_json(
        "Microsoft (R) Library Manager Version 14.39.33523.0\nCopyright (C) Microsoft Corporation.\n",
    );
    let expected = r#"{
  "identity": {
    "kind": "lib",
    "version": "14.39.33523",
    "raw_version_line": "Microsoft (R) Library Manager Version 14.39.33523.0"
  },
  "capabilities": {
    "ar_crs": {
      "supported": false,
      "source": "unsupported"
    },
    "static_library_output": {
      "supported": true,
      "source": "version"
    }
  }
}"#;
    assert_eq!(actual, expected);
}

#[test]
fn snapshot_full_detection_report_for_clang_plus_gnu_ar() {
    // End-to-end snapshot of `ToolchainDetectionReport::as_json`
    // for a typical Linux clang + GNU ar setup. Pins the
    // top-level shape `{ cxx, [cc,] ar }` plus all nested
    // fields in their insertion order.
    let cxx_id =
        parse_cxx_version_output("clang version 17.0.6\nTarget: x86_64-unknown-linux-gnu\n");
    let cxx_caps = derive_cxx_capabilities(&cxx_id);
    let ar_id = parse_ar_version_output("GNU ar (GNU Binutils) 2.40\n");
    let ar_caps = derive_ar_capabilities(&ar_id);
    let report = ToolchainDetectionReport {
        cxx: ToolDetection {
            path: camino::Utf8PathBuf::from("/opt/llvm/bin/clang++"),
            identity: cxx_id,
            capabilities: cxx_caps,
        },
        cc: None,
        ar: ToolDetection {
            path: camino::Utf8PathBuf::from("/usr/bin/ar"),
            identity: ar_id,
            capabilities: ar_caps,
        },
    };
    let actual = pretty(&report.as_json());
    let expected = r#"{
  "cxx": {
    "path": "/opt/llvm/bin/clang++",
    "identity": {
      "kind": "clang",
      "version": "17.0.6",
      "target": "x86_64-unknown-linux-gnu",
      "raw_version_line": "clang version 17.0.6"
    },
    "capabilities": {
      "c_standard_11": {
        "supported": true,
        "source": "version"
      },
      "cxx_standard_17": {
        "supported": true,
        "source": "version"
      },
      "depfile_mmd_mf": {
        "supported": true,
        "source": "version"
      },
      "gcc_style_flags": {
        "supported": true,
        "source": "version"
      },
      "msvc_style_flags": {
        "supported": false,
        "source": "assumed-default"
      },
      "std_flag": {
        "supported": true,
        "source": "version"
      }
    }
  },
  "ar": {
    "path": "/usr/bin/ar",
    "identity": {
      "kind": "ar",
      "version": "2.40",
      "raw_version_line": "GNU ar (GNU Binutils) 2.40"
    },
    "capabilities": {
      "ar_crs": {
        "supported": true,
        "source": "version"
      },
      "static_library_output": {
        "supported": true,
        "source": "version"
      }
    }
  }
}"#;
    assert_eq!(actual, expected);
}

struct CxxBannerCase {
    banner: &'static str,
    kind: CompilerKind,
    version: Option<(u32, Option<u32>, Option<u32>)>,
    target: Option<&'static str>,
}

/// Real captured `--version` first lines (plus follow-up lines where
/// the real tool prints them) across families, versions, and vendor
/// patches. Every parser change must keep this table green.
const CXX_BANNER_CORPUS: &[CxxBannerCase] = &[
    // --- GCC ---
    CxxBannerCase {
        banner: "g++ (Ubuntu 11.4.0-1ubuntu1~22.04) 11.4.0\nCopyright (C) 2021 Free Software Foundation, Inc.\n",
        kind: CompilerKind::Gcc,
        version: Some((11, Some(4), Some(0))),
        target: None,
    },
    CxxBannerCase {
        banner: "g++ (Ubuntu 13.3.0-6ubuntu2~24.04) 13.3.0\n",
        kind: CompilerKind::Gcc,
        version: Some((13, Some(3), Some(0))),
        target: None,
    },
    CxxBannerCase {
        banner: "g++ (Debian 12.2.0-14) 12.2.0\n",
        kind: CompilerKind::Gcc,
        version: Some((12, Some(2), Some(0))),
        target: None,
    },
    CxxBannerCase {
        banner: "g++ (GCC) 14.2.1 20240910\n",
        kind: CompilerKind::Gcc,
        version: Some((14, Some(2), Some(1))),
        target: None,
    },
    CxxBannerCase {
        banner: "c++ (GCC) 8.5.0 20210514 (Red Hat 8.5.0-26)\n",
        kind: CompilerKind::Gcc,
        version: Some((8, Some(5), Some(0))),
        target: None,
    },
    CxxBannerCase {
        banner: "g++ (Alpine 13.2.1_git20231014) 13.2.1 20231014\n",
        kind: CompilerKind::Gcc,
        version: Some((13, Some(2), Some(1))),
        target: None,
    },
    CxxBannerCase {
        banner: "g++ (Homebrew GCC 14.2.0_1) 14.2.0\n",
        kind: CompilerKind::Gcc,
        version: Some((14, Some(2), Some(0))),
        target: None,
    },
    CxxBannerCase {
        banner: "g++.exe (Rev3, Built by MSYS2 project) 13.2.0\n",
        kind: CompilerKind::Gcc,
        version: Some((13, Some(2), Some(0))),
        target: None,
    },
    CxxBannerCase {
        banner: "g++-14 (Homebrew GCC 14.2.0) 14.2.0\n",
        kind: CompilerKind::Gcc,
        version: Some((14, Some(2), Some(0))),
        target: None,
    },
    CxxBannerCase {
        banner: "g++ (GCC) 7.3.1 20180303 (Red Hat 7.3.1-5)\n",
        kind: CompilerKind::Gcc,
        version: Some((7, Some(3), Some(1))),
        target: None,
    },
    CxxBannerCase {
        banner: "g++ (SUSE Linux) 7.5.0\n",
        kind: CompilerKind::Gcc,
        version: Some((7, Some(5), Some(0))),
        target: None,
    },
    CxxBannerCase {
        banner: "g++ (MinGW-W64 i686-ucrt-posix-dwarf, built by Brecht Sanders) 13.2.0\n",
        kind: CompilerKind::Gcc,
        version: Some((13, Some(2), Some(0))),
        target: None,
    },
    CxxBannerCase {
        banner: "g++ (GCC) 15.1.0\n",
        kind: CompilerKind::Gcc,
        version: Some((15, Some(1), Some(0))),
        target: None,
    },
    CxxBannerCase {
        banner: "aarch64-linux-gnu-g++ (Ubuntu 11.4.0-1ubuntu1~22.04) 11.4.0\nCopyright (C) 2021 Free Software Foundation, Inc.\n",
        kind: CompilerKind::Gcc,
        version: Some((11, Some(4), Some(0))),
        target: None,
    },
    // --- LLVM Clang (plain + distro-patched + suffixed) ---
    CxxBannerCase {
        banner: "clang version 17.0.6 (Fedora 17.0.6-2.fc39)\nTarget: x86_64-redhat-linux-gnu\nThread model: posix\n",
        kind: CompilerKind::Clang,
        version: Some((17, Some(0), Some(6))),
        target: Some("x86_64-redhat-linux-gnu"),
    },
    CxxBannerCase {
        banner: "Ubuntu clang version 18.1.3 (1ubuntu1)\nTarget: x86_64-pc-linux-gnu\n",
        kind: CompilerKind::Clang,
        version: Some((18, Some(1), Some(3))),
        target: Some("x86_64-pc-linux-gnu"),
    },
    CxxBannerCase {
        banner: "Debian clang version 16.0.6 (15~deb12u1)\n",
        kind: CompilerKind::Clang,
        version: Some((16, Some(0), Some(6))),
        target: None,
    },
    CxxBannerCase {
        banner: "clang version 3.9.1 (tags/RELEASE_391/final)\n",
        kind: CompilerKind::Clang,
        version: Some((3, Some(9), Some(1))),
        target: None,
    },
    CxxBannerCase {
        banner: "clang version 10.0.0-4ubuntu1 \nTarget: x86_64-pc-linux-gnu\n",
        kind: CompilerKind::Clang,
        version: Some((10, Some(0), Some(0))),
        target: Some("x86_64-pc-linux-gnu"),
    },
    CxxBannerCase {
        banner: "clang version 20.1.0git (https://github.com/llvm/llvm-project 4a0aff96d7e9695be96b1429e1d1f9e305766b0a)\n",
        kind: CompilerKind::Clang,
        version: Some((20, Some(1), Some(0))),
        target: None,
    },
    CxxBannerCase {
        banner: "FreeBSD clang version 18.1.6 (https://github.com/llvm/llvm-project.git llvmorg-18.1.6-0-g1118c2e05e67)\nTarget: x86_64-unknown-freebsd14.1\n",
        kind: CompilerKind::Clang,
        version: Some((18, Some(1), Some(6))),
        target: Some("x86_64-unknown-freebsd14.1"),
    },
    CxxBannerCase {
        banner: "Homebrew clang version 19.1.7\nTarget: arm64-apple-darwin24.3.0\n",
        kind: CompilerKind::Clang,
        version: Some((19, Some(1), Some(7))),
        target: Some("arm64-apple-darwin24.3.0"),
    },
    CxxBannerCase {
        banner: "OpenBSD clang version 16.0.6\nTarget: amd64-unknown-openbsd7.5\n",
        kind: CompilerKind::Clang,
        version: Some((16, Some(0), Some(6))),
        target: Some("amd64-unknown-openbsd7.5"),
    },
    CxxBannerCase {
        banner: "clang version 19.1.0-rc2\n",
        kind: CompilerKind::Clang,
        version: Some((19, Some(1), Some(0))),
        target: None,
    },
    // clang-cl prints a plain clang banner; the detector reclassifies
    // by invoked name (cabin-toolchain), so banner-level kind is Clang.
    CxxBannerCase {
        banner: "clang version 18.1.8\nTarget: x86_64-pc-windows-msvc\nThread model: posix\n",
        kind: CompilerKind::Clang,
        version: Some((18, Some(1), Some(8))),
        target: Some("x86_64-pc-windows-msvc"),
    },
    // --- Apple clang (Xcode), incl. the pre-Xcode-10 "Apple LLVM" era ---
    CxxBannerCase {
        banner: "Apple clang version 16.0.0 (clang-1600.0.26.4)\nTarget: arm64-apple-darwin24.0.0\nThread model: posix\n",
        kind: CompilerKind::AppleClang,
        version: Some((16, Some(0), Some(0))),
        target: Some("arm64-apple-darwin24.0.0"),
    },
    CxxBannerCase {
        banner: "Apple clang version 11.0.3 (clang-1103.0.32.62)\n",
        kind: CompilerKind::AppleClang,
        version: Some((11, Some(0), Some(3))),
        target: None,
    },
    CxxBannerCase {
        banner: "Apple LLVM version 10.0.1 (clang-1001.0.46.4)\nTarget: x86_64-apple-darwin18.5.0\n",
        kind: CompilerKind::AppleClang,
        version: Some((10, Some(0), Some(1))),
        target: Some("x86_64-apple-darwin18.5.0"),
    },
    CxxBannerCase {
        banner: "Apple LLVM version 9.1.0 (clang-902.0.39.1)\n",
        kind: CompilerKind::AppleClang,
        version: Some((9, Some(1), Some(0))),
        target: None,
    },
    // --- MSVC (x86/x64/ARM64 tails, two-digit minor, localized) ---
    CxxBannerCase {
        banner: "Microsoft (R) C/C++ Optimizing Compiler Version 19.29.30153 for x64\nCopyright (C) Microsoft Corporation.  All rights reserved.\n",
        kind: CompilerKind::Msvc,
        version: Some((19, Some(29), Some(30153))),
        target: None,
    },
    CxxBannerCase {
        banner: "Microsoft (R) C/C++ Optimizing Compiler Version 19.40.33811 for ARM64\n",
        kind: CompilerKind::Msvc,
        version: Some((19, Some(40), Some(33811))),
        target: None,
    },
    CxxBannerCase {
        banner: "Microsoft (R) C/C++ Optimizing Compiler Version 19.00.24210 for x86\n",
        kind: CompilerKind::Msvc,
        version: Some((19, Some(0), Some(24210))),
        target: None,
    },
    CxxBannerCase {
        banner: "Microsoft (R) C/C++-Optimierungscompiler Version 19.29.30133 f\u{fc}r x64\n",
        kind: CompilerKind::Msvc,
        version: Some((19, Some(29), Some(30133))),
        target: None,
    },
    // --- Unrecognized compilers stay Unknown (closed family set) ---
    CxxBannerCase {
        banner: "Intel(R) oneAPI DPC++/C++ Compiler 2025.0.0 (2025.0.0.20241008)\n",
        kind: CompilerKind::Unknown,
        version: None,
        target: None,
    },
    CxxBannerCase {
        banner: "nvc++ 24.7-0 64-bit target on x86-64 Linux -tp haswell\n",
        kind: CompilerKind::Unknown,
        version: None,
        target: None,
    },
    CxxBannerCase {
        banner: "tcc version 0.9.27 (x86_64 Linux)\n",
        kind: CompilerKind::Unknown,
        version: None,
        target: None,
    },
    CxxBannerCase {
        banner: "cc: error: no input files\n",
        kind: CompilerKind::Unknown,
        version: None,
        target: None,
    },
];

#[test]
fn cxx_banner_corpus() {
    for case in CXX_BANNER_CORPUS {
        let id = parse_cxx_version_output(case.banner);
        assert_eq!(
            id.kind, case.kind,
            "kind mismatch for banner {:?}",
            case.banner
        );
        let got = id.version.as_ref().map(|v| (v.major, v.minor, v.patch));
        assert_eq!(
            got, case.version,
            "version mismatch for banner {:?}",
            case.banner
        );
        assert_eq!(
            id.target.as_deref(),
            case.target,
            "target mismatch for banner {:?}",
            case.banner
        );
    }
}

struct ArBannerCase {
    banner: &'static str,
    kind: ArchiverKind,
    version: Option<(u32, Option<u32>, Option<u32>)>,
}

const AR_BANNER_CORPUS: &[ArBannerCase] = &[
    ArBannerCase {
        banner: "GNU ar (GNU Binutils for Ubuntu) 2.42\n",
        kind: ArchiverKind::Ar,
        version: Some((2, Some(42), None)),
    },
    ArBannerCase {
        banner: "GNU ar (GNU Binutils for Debian) 2.40\n",
        kind: ArchiverKind::Ar,
        version: Some((2, Some(40), None)),
    },
    ArBannerCase {
        banner: "LLVM (http://llvm.org/):\n  LLVM version 17.0.6\n  Optimized build.\n",
        kind: ArchiverKind::LlvmAr,
        version: Some((17, Some(0), Some(6))),
    },
    ArBannerCase {
        banner: "Microsoft (R) Library Manager Version 14.40.33811.0\nCopyright (C) Microsoft Corporation.  All rights reserved.\n",
        kind: ArchiverKind::Lib,
        version: Some((14, Some(40), Some(33811))),
    },
    ArBannerCase {
        banner: "not an archiver\n",
        kind: ArchiverKind::Unknown,
        version: None,
    },
];

#[test]
fn ar_banner_corpus() {
    for case in AR_BANNER_CORPUS {
        let id = parse_ar_version_output(case.banner);
        assert_eq!(
            id.kind, case.kind,
            "kind mismatch for banner {:?}",
            case.banner
        );
        let got = id.version.as_ref().map(|v| (v.major, v.minor, v.patch));
        assert_eq!(
            got, case.version,
            "version mismatch for banner {:?}",
            case.banner
        );
    }
}

#[test]
fn parse_with_suffix_strips_trailing_garbage_per_component() {
    for (raw, expect) in [
        ("20.1.0git", (20, Some(1), Some(0))),
        ("10.0.0-4ubuntu1", (10, Some(0), Some(0))),
        ("19.1.0-rc2", (19, Some(1), Some(0))),
        ("14.2.1", (14, Some(2), Some(1))),
        ("2.42", (2, Some(42), None)),
    ] {
        let v = CompilerVersion::parse_with_suffix(raw).expect(raw);
        assert_eq!((v.major, v.minor, v.patch), expect, "{raw}");
        assert_eq!(v.raw, raw);
    }
    assert!(CompilerVersion::parse_with_suffix("garbage").is_none());
    assert!(CompilerVersion::parse_with_suffix("v14.2").is_none());
}
