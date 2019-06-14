> 日本語版は[こちら](https://doc.poac.pm/ja/guide/creating-a-new-project.html)

## Creating a new project

First, create a new project by executing the `poac new` command in the same way as [1.2. Hello World](../getting-started/hello-world.md).

```bash
$ poac new hello
Created: application `hello` project
Running: git init hello
```

When you move to the created directory, the file `poac.yml` is created.
At the end of this file, add the following content.
Specify the list of dependent packages used by the current hello project as the deps key.
In this case, it means using the version of 0.1.0 or more and less than 1.0.0 of the hello_world package.

```yaml
deps:
  hello_world: ">=0.1.0 and <1.0.0"
```

After editing, execute the `poac install` command to install it.

```bash
$ poac install
==> Resolving packages...
==> Resolving dependencies...
==> Fetching...

  ●  hello_world 0.1.0 (from: poac)

==> Done.
```

*If you execute `poac install hello_world` command, it will be installed automatically, even if you do not edit poac.yml.*


Edit `main.cpp`.

```cpp
#include <iostream>
#include <hello_world.hpp>

int main(int argc, char** argv) {
   hello_world::say();
}
```

Finally, when you execute `poac run`, `Hello, world!` is displayed, and it has been checked that can use the external package!

```bash
$ poac run
Compiled: Output to `_build/bin/hello`
Running: `_build/bin/hello`
Hello, world!
```
