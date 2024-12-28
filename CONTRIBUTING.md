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
standards by running the following tools:

1. Run the linter (`cpplint`)
   ```bash
   cabin lint --exclude srcOld --exclude testsOld
   ```
2. Run the formatter (`clang-format`)
   ```bash
   cabin fmt --exclude srcOld --exclude testsOld
   ```
3. Run the static analyzer (`clang-tidy`)
   ```bash
   cabin tidy  # or make tidy
   ```

### Testing

Always validate your changes to ensure they do not introduce regressions or
break existing functionality:

```bash
# Unit tests
cabin test  # or make test

# Integration tests
wget https://raw.githubusercontent.com/felipec/sharness/refs/tags/v1.2.1/sharness.sh
mv sharness.sh tests/
prove -j$(nproc) --shuffle tests/[0-9]*.sh
```

Make sure to add new tests for any new functionality you introduce.

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

We generally follow the
[Git Contribution Guidelines](https://git-scm.com/docs/SubmittingPatches#describe-changes).

- **First Line**: `component: description of the patch` in all lowercase.
- **Body**: Explain your change in 2-3 paragraphs:
  1. the current problem you want to solve
  2. justification of your change
  3. if any, alternative solutions considered but discarded

Read [Git Contribution Guidelines](https://git-scm.com/docs/SubmittingPatches#describe-changes)
for more examples.

## Pull Request Style

1. **Title**: Follow the First Line rule of [Commit Message](#commit-message).
2. **Description**: Follow the Body rule of [Commit Message](#commit-message).
3. **CI**: Verify that all CI checks pass on your fork before submitting the
   PR.  Avoid relying on the CI of this repository to catch errors, as this
   can cause delays or stalls for other contributors.
4. **Commits**: There is no need to squash commits within the PR
   unless explicitly requested.  Keeping separate commits can help reviewers
   understand the progression of changes.
