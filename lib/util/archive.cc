// external
#include <boost/scope_exit.hpp>
#include <spdlog/spdlog.h> // NOLINT(build/include_order)

// internal
#include "poac/util/archive.hpp"

namespace poac::util::archive {

[[nodiscard]] Result<void, String>
archive_write_data_block(
    const Writer& writer, const void* buffer, usize size, i64 offset
) noexcept {
  const la_ssize_t res =
      archive_write_data_block(writer.get(), buffer, size, offset);
  if (res < ARCHIVE_OK) {
    return Err(archive_error_string(writer.get()));
  }
  return Ok();
}

[[nodiscard]] Result<void, String>
copy_data(Archive* reader, const Writer& writer) noexcept {
  usize size{};
  const void* buff = nullptr;
  i64 offset{};

  while (true) {
    const i32 res = archive_read_data_block(reader, &buff, &size, &offset);
    if (res == ARCHIVE_EOF) {
      return Ok();
    } else if (res < ARCHIVE_OK) {
      return Err(archive_error_string(reader));
    }
    Try(archive_write_data_block(writer, buff, size, offset));
  }
}

[[nodiscard]] Result<void, String>
archive_write_finish_entry(const Writer& writer) noexcept {
  const i32 res = archive_write_finish_entry(writer.get());
  if (res < ARCHIVE_OK) {
    return Err(archive_error_string(writer.get()));
  } else if (res < ARCHIVE_WARN) {
    return Err("Encountered error while finishing entry.");
  }
  return Ok();
}

[[nodiscard]] Result<void, String>
archive_write_header(
    Archive* reader, const Writer& writer, archive_entry* entry
) noexcept {
  if (archive_write_header(writer.get(), entry) < ARCHIVE_OK) {
    return Err(archive_error_string(writer.get()));
  } else if (archive_entry_size(entry) > 0) {
    Try(copy_data(reader, writer));
  }
  return Ok();
}

String
set_extract_path(archive_entry* entry, const Path& extract_path) noexcept {
  String current_file = archive_entry_pathname(entry);
  const Path full_output_path = extract_path / current_file;
  log::debug("extracting to `{}`", full_output_path.string());
  archive_entry_set_pathname(entry, full_output_path.c_str());
  return current_file;
}

[[nodiscard]] Result<bool, String>
archive_read_next_header_(Archive* reader, archive_entry** entry) noexcept(
    !static_cast<bool>(ARCHIVE_EOF) // NOLINT(modernize-use-bool-literals)
) {
  const i32 res = archive_read_next_header(reader, entry);
  if (res == ARCHIVE_EOF) {
    return Ok(ARCHIVE_EOF);
  } else if (res < ARCHIVE_OK) {
    return Err(archive_error_string(reader));
  } else if (res < ARCHIVE_WARN) {
    return Err("Encountered error while reading header.");
  }
  return Ok(false);
}

[[nodiscard]] Result<String, String>
extract_impl(
    Archive* reader, const Writer& writer, const Path& extract_path
) noexcept {
  archive_entry* entry = nullptr;
  String extracted_directory_name;
  while (Try(archive_read_next_header_(reader, &entry)) != ARCHIVE_EOF) {
    if (extracted_directory_name.empty()) {
      extracted_directory_name = set_extract_path(entry, extract_path);
    } else {
      set_extract_path(entry, extract_path);
    }
    Try(archive_write_header(reader, writer, entry));
    Try(archive_write_finish_entry(writer));
  }
  return Ok(extracted_directory_name);
}

[[nodiscard]] Result<void, String>
archive_read_open_filename(
    Archive* reader, const Path& file_path, usize block_size
) noexcept {
  if (archive_read_open_filename(reader, file_path.c_str(), block_size)) {
    return Err("Cannot archive_read_open_filename");
  }
  return Ok();
}

[[nodiscard]] Result<String, String>
extract(const Path& target_file_path, const Path& extract_path) noexcept {
  Archive* reader = archive_read_new();
  if (!reader) {
    return Err("Cannot archive_read_new");
  }
  BOOST_SCOPE_EXIT_ALL(&reader) { archive_read_free(reader); };
  read_as_targz(reader);

  Writer writer(archive_write_disk_new());
  if (!writer) {
    return Err("Cannot archive_write_disk_new");
  }

  archive_write_disk_set_options(writer.get(), make_flags());
  archive_write_disk_set_standard_lookup(writer.get());

  Try(archive_read_open_filename(reader, target_file_path, 10'240));
  BOOST_SCOPE_EXIT_ALL(&reader) { archive_read_close(reader); };

  return extract_impl(reader, writer, extract_path);
}

} // namespace poac::util::archive
