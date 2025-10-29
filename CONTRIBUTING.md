# Contributing

Given our limited resources, we may not review PRs that fail to adhere to this document.

Note that we have a [code of conduct](https://github.com/cabinpkg/.github/blob/main/CODE_OF_CONDUCT.md),
follow it in all your interactions with the project.

You can ignore sections marked as "Under Construction".

## How to Contribute

You can contribute to this repository in one of the following ways:

1. **For Trivial Changes**<br>
   If you believe your change is minor (e.g., typo fixes, small improvements),
   feel free to make the change directly and submit a pull request (PR).
2. **For Uncertain Changes**<br>
   If you're unsure whether your proposed change aligns with the project's
   goals, we encourage you to discuss it first by opening an issue or starting
   a discussion.  This helps ensure alignment and reduces potential rework.
3. **For Exploratory Contributions**<br>
   If you're unsure but find it easier to share code, you can create a draft PR
   with the prefix `RFC:` in the title (e.g., `RFC: build: add new feature X`).
   This signals that the PR is a "Request for Comments" and invites feedback
   from the maintainers and community.

## Coding Style

Consistency is key to maintaining a clean and readable codebase. As stated in the
[LLVM Coding Standards](https://llvm.org/docs/CodingStandards.html#introduction):

> **If you are extending, enhancing, or bug fixing already implemented code,
> use the style that is already being used so that the source is uniform and
> easy to follow.**

Please follow this principle to ensure the code remains cohesive and easy to
navigate.

### Naming Conventions (*Under Construction)

The project's naming conventions are specified in the
[.clang-tidy](.clang-tidy) file.  Here's a brief summary:

- **Files/Directories**: `PascalCase`
- **Types/Classes**: `PascalCase`
- **Variables**: `snake_case`
- **Class (non-struct) Member Variables**: `snake_case_`
- **Functions**: `camelCase`
- **Class (non-struct) Methods**: `camelCase_`

(Note: Variables use `snake_case` since they tend to be shorter than functions.)

Be mindful of when to use structs vs. classes.  For guidance, refer to the
[Google C++ Style Guide: Structs vs. Classes](https://google.github.io/styleguide/cppguide.html#Structs_vs._Classes).

### Formatting and Linting

Before submitting a PR, ensure your code adheres to the project's coding
standards.  Also, always validate your changes to ensure they do not
introduce regressions or break existing functionality:

1. Run the linter (`cpplint`)
   ```bash
   ./build/cabin lint
   ```
2. Run the formatter (`clang-format`)
   ```bash
   ./build/cabin fmt
   ```
3. Run the static analyzer (`clang-tidy`)
   ```bash
   ./build/cabin tidy
   ```
4. Run tests
   ```bash
   ./build/cabin test
   ```

### Packaging

To test the Linux package creation locally, you can use nFPM:

```bash
# Install nFPM (if not already installed)
brew install nfpm  # macOS
# or follow instructions at https://nfpm.goreleaser.com/install/

# Build cabin first
make BUILD=release -j$(nproc)

# Create DEB package
CABIN_VERSION="1.0.0" nfpm pkg --packager deb

# Create RPM package
CABIN_VERSION="1.0.0" nfpm pkg --packager rpm

# Verify packages were created
ls -la *.deb *.rpm
```

The packaging configuration is defined in `nfpm.yaml`.  Both DEB and RPM
packages are automatically created and published on GitHub releases when tags
are pushed.

## Documentation

If your changes affect the project's documentation, ensure you update the
relevant files in the `docs/` directory.  You can preview your changes by
running the following command:

```bash
pip install mkdocs
mkdocs serve
```

This will start a local server at `http://127.0.0.1:8000/`.

Make sure to update the table of contents in the `mkdocs.yml` file to reflect
your changes.  Also, ensure that the documentation is clear, concise, and
formatted correctly.

Before committing anything, ensure that the documentation builds without
errors by running:

```bash
mkdocs build --strict
```

## Commit Message

Follow [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/).

## Pull Request Style

1. **CI**: Verify that all CI checks pass on your fork before submitting the
   PR.  Avoid relying on the CI of this repository to catch errors, as this
   can cause delays or stalls for other contributors.
2. **Commits**: There is no need to squash / force push commits within the PR
   unless explicitly requested.  Keeping separate commits can help reviewers
   understand the progression of changes.
