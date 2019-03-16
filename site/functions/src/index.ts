import handler from "./handlers/handler";

// This line is important. What we are doing here
//  is exporting ONE function with the name
//  `api` which will always route
exports.onMetadataCreated = handler.onMetadataCreated;
exports.onMetadataDeleted = handler.onMetadataDeleted;
