# Usage

## Start a new project with Cabin

The `cabin new` command lets you start a new Cabin project:

```console
you:~$ cabin new hello_world
     Created binary (application) `hello_world` package
```

> [!TIP]
> If you want to integrate your existing project with Cabin, use the `init` command:
>
> ```console
> you:~/your-pj$ cabin init
>      Created binary (application) `your-pj` package
> ```
>
> This command just creates a `cabin.toml` file not to break your project.

## Build the project

In most cases, you want to execute the generated binary as well as build the project.

```console
you:~/hello_world$ cabin run
 Compiling src/main.cc
   Linking hello_world
  Finished debug target(s) in 0.45386s
   Running cabin-out/debug/hello_world
Hello, world!
```

If you just want to build it, run the `build` command:

```console
you:~/hello_world$ cabin build
  Finished debug target(s) in 0.00866317s
```

Cabin uses a cache since we executed the command with no changes.

> [!TIP]
> To use a different compiler, you can export a `CXX` environmental variable:
>
> ```shell
> export CXX=g++-13
> ```

## Install dependencies

Like Cargo does, Cabin installs dependencies at build time.  Cabin currently supports Git, path, and system dependencies.  You can use two ways to add dependencies to your project: using the `cabin add` command and editing `cabin.toml` directly.

> [!WARNING]
> - Cabin currently supports building a project with header-only dependencies.
> - Building with build-required dependencies will be soon supported.

> [!NOTE]  
> A header-only library is a type of library where the entire functionality is contained within header files, and no additional compilation steps are required. This means you can directly include the library's headers in your project without needing to link against any compiled binaries.  
>  
> These libraries require the headers to be in the root or include/ directory. This ensures that the library's header files are accessible in a predictable location for easy inclusion in your project.  
>  
> Here are some links that can help you find header-only libraries:  
> - https://github.com/topics/single-header  
> - https://github.com/topics/header-only  
> - https://github.com/p-ranav/awesome-hpp

### `cabin add`

The `cabin add` command accepts the following arguments:

`cabin add <package names ....> --<options>`

Options:
- `--sys`: Marks the packages as system dependency (requires the `--version` argument)
- `--version`: Specify dependency version. Only used with system dependencies
- `--rev`: Specify revision for git dependencies
- `--tag`: Specify tag for git dependencies
- `--branch`: Specify branch for git dependencies

Example
```bash
cabin add libgit2 --sys --version "1.1.0"
cabin add "ToruNiina/toml11" --rev "846abd9a49082fe51440aa07005c360f13a67bbf"
```

### Editing `cabin.toml` directly

The syntax for `cabin.toml` is as follows:

```toml
[dependencies]

# git dependency
"ToruNiina/toml11" = { git = "https://github.com/ToruNiina/toml11.git", rev = "846abd9a49082fe51440aa07005c360f13a67bbf" }

# path dependency
local_lib = { path = "../local_lib" }

# system dependency
fmt = { version = ">= 9", system = true }
```

If `tag`, `branch`, or `rev` is unspecified for git dependencies, Cabin will use the latest revision of the default branch. System dependency names must be acceptable by `pkg-config`. The version requirement syntax is specified in [src/VersionReq.hpp](https://github.com/cabinpkg/cabin/blob/main/src/VersionReq.hpp).

After adding dependencies, executing the `build` command will install the package and its dependencies.

```console
you:~/hello_world$ cabin build
Downloaded ToruNiina/toml11 846abd9a49082fe51440aa07005c360f13a67bbf
 Compiling src/main.cc
   Linking hello_world
  Finished debug target(s) in 0.70s
```

### Remove dependencies

Use the `remove` command to remove dependencies from cabin.toml:

```console
you:~/hello_world$ cabin remove my_dep
    Removed my_dep from cabin.toml
```

## Unit tests

You can write unit tests in any source files within the `src` directory.  Create a new file like:

`src/Lib.cc`

```cpp
int add(int a, int b) {
  return a + b;
}

#ifdef CABIN_TEST

#  include <cassert>

int main() {
  assert(add(1, 2) == 3);  // ok
  assert(add(1, 2) == 4);  // fail
}

#endif
```

Now, with the `test` command, you can run tests defined within `CABIN_TEST`:

```console
you:~/hello_world$ cabin test
 Compiling src/Lib.cc
   Linking tests/test_Lib
   Testing Lib
Assertion failed: (add(1, 2) == 4), function main, file Lib.cc, line 13.
make: *** [test] Abort trap: 6
```

Unit tests with the `CABIN_TEST` macro are useful when testing private functions.  Integration testing with the `tests` directory has not yet been implemented.

## Run linter

Linting source code is essential to protect its quality.  Cabin supports linting your project by the `lint` command:

```console
you:~/hello_world$ cabin lint
   Linting hello_world
src/main.cc:0:  No copyright message found.  You should have a line: "Copyright [year] <Copyright Owner>"  [legal/copyright] [5]
Done processing src/main.cc
Total errors found: 1
Error: `cpplint` exited with status 1
```

> [!TIP]
> If you do not have `cpplint`, install it with the following command:
>
> ```bash
> pip install cpplint
> ```

The `lint` command works without configurations, and Cabin would automatically opt out of unwanted lints by adjusting to each project.
To customize the lint settings, try adding the `[lint.cpplint]` key in your `cabin.toml` like [this](https://github.com/cabinpkg/cabin/blob/cc30b706fb49860903384df56d650a0955aca16c/cabin.toml#L67-L83)
or creating a [`CPPLINT.cfg`](https://github.com/cabinpkg/cabin/blob/5e7e3792e8818d165149214e94f30958fb0fef66/CPPLINT.cfg) file in the repository root.

## Run formatter

Cabin also supports formatting your source code with `clang-format`.  Ensure having installed `clang-format` before running this command.

```console
you:~/hello_world$ cabin fmt
  Formatting hello_world
```

> [!NOTE]
> This command automatically detects what files we need to format to avoid getting bothered by commands like:
>
> ```console
> $ # We need to avoid the `build` dir and such dirs ...
> $ clang-format ./src/*.cpp -i
> $ clang-format ./include/**/*.hpp -i
> $ clang-format ./tests/**/*.cpp -i
> $ ...
> ```

To customize the format settings, try creating a [`.clang-format`](https://github.com/cabinpkg/cabin/blob/main/.clang-format) file to the repository root.

## Run `clang-tidy`

Cabin also supports running `clang-tidy` on your source code.  Ensure having installed `clang-tidy` before running this command.

```console
you:~/hello_world$ cabin tidy
  Running clang-tidy
```

You can customize the tidy settings by creating a [`.clang-tidy`](https://github.com/cabinpkg/cabin/blob/main/.clang-tidy) file to the repository root.
