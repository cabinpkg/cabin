import app from "./common/server";
import handler from "./handlers/handler";
import { functions } from "./common/firebase";

// This line is important. What we are doing here
//  is exporting ONE function with the name
//  `api` which will always route
exports.api = functions.https.onRequest(app);
exports.onMetadataCreated = handler.onMetadataCreated;
exports.onMetadataUpdated = handler.onMetadataUpdated;
exports.onMetadataDeleted = handler.onMetadataDeleted;
