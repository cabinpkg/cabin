> 日本語版は[こちら](https://doc.poac.pm/ja/getting-started/hello-world.html)

## Hello World

### Start a new project with Poac

Use this command when you start a new poac project.

```bash
$ poac create hello_world
     Created binary (application) `hello_world` package
```

> If you want to integrate your existing project with Poac, use the `init` command:
>
> ```bash
> your-pj/$ poac init
>      Created binary (application) `your-pj` package
> ```
>
> This command just creates a `poac.toml` file not to let your project break.
### Build the project

In most cases, you will want to execute as well as build—of course, you can.

```bash
hello_world/$ poac run
   Compiling 1/1: hello_world v0.1.0 (/Users/ken-matsui/hello_world)
    Finished debug target(s) in 0.90s
     Running `/Users/ken-matsui/hello_world/poac_output/debug/hoge`
Hello, world!
```

Should you just build it, run the `build` command:

```bash
hello_world/$ poac build
    Finished debug target(s) in 0.21s
```

Poac uses a cache since we executed the command with no changes.
