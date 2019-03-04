
```bash
$ poac new hello

Your "hello" project was created successfully.


Go into your project by running:
    $ cd hello

Start your project with:
    $ poac run
```

```bash
$ cd hello
$ vim poac.yml
```

```yaml
...
deps:
  hello_world: 0.1.0
...
```

```bash
$ poac install
Some new packages are needed.

 ✔  Installed! (from poac)             hello_world: 0.1.0

Elapsed time: 0.106303s
 ==> Installation finished successfully!

$ vim main.cpp
```

```cpp
#include <iostream>
#include <hello_world.hpp>

int main(int argc, char** argv) {
   hello_world::say();
}
```

```bash
$ poac run
Compiled: Output to `_build/bin/hello`
Running: `_build/bin/hello`
Hello, world!
```
