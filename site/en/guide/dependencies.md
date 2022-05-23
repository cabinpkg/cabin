> 日本語版は[こちら](https://doc.poac.pm/ja/guide/dependencies.html)

Poac currently supports two types of dependencies: dependencies and dev-dependencies.

At build time, Poac will install these dependencies.

## Dependencies

These dependencies are exposed to other packages which depend on this package.

```toml
[dependencies]
"boost/config": ">=1.64.0 and <2.0.0"
```

## Development dependencies

Development dependencies are used for internal use, such as tests, examples, and benchmarks.
These dependencies are not propagated to other packages which depend on this package.

```toml
[dev-dependencies]
"boost/predef" = ">=1.64.0 and <2.0.0"
```
