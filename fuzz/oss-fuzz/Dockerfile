# OSS-Fuzz project files (../README.md, "OSS-Fuzz"); Cabin is not
# enrolled.
FROM gcr.io/oss-fuzz-base/base-builder-rust
# The last nightly on LLVM 22, which OSS-Fuzz's coverage tools read; the
# image's own predates Cabin's MSRV.
ENV RUSTUP_TOOLCHAIN=nightly-2026-08-05
RUN rustup toolchain install $RUSTUP_TOOLCHAIN --profile minimal
RUN git clone --depth 1 https://github.com/cabinpkg/cabin cabin
WORKDIR $SRC/cabin
COPY build.sh $SRC/
