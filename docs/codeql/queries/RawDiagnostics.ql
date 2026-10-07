/** Exports all located diagnostics, including information and warning severity. */

import rust

from
  @diagnostic id, int severity, string tag, string message, string fullMessage,
  @location_default location, @file file, string name, int beginLine, int beginColumn, int endLine,
  int endColumn
where
  diagnostics(id, severity, tag, message, fullMessage, location) and
  locations_default(location, file, beginLine, beginColumn, endLine, endColumn) and
  files(file, name)
select name, beginLine, beginColumn, severity, tag, message, fullMessage
