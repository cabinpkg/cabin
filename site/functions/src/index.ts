import * as algoliasearch from "algoliasearch";
import app from "./server";
import { functions } from "./firebase";


// This line is important. What we are doing here
//  is exporting ONE function with the name
//  `api` which will always route
exports.api = functions.https.onRequest(app);



// Initialize Algolia, requires installing Algolia dependencies:
// https://www.algolia.com/doc/api-client/javascript/getting-started/#install
//
// App ID and API Key are stored in functions config variables
const ALGOLIA_ID = functions.config().algolia.app_id;
const ALGOLIA_ADMIN_KEY = functions.config().algolia.api_key;
// const ALGOLIA_SEARCH_KEY = functions.config().algolia.search_key;

const ALGOLIA_INDEX_NAME = 'packages';
const client = algoliasearch(ALGOLIA_ID, ALGOLIA_ADMIN_KEY);

// Update the search index every time a blog post is written.
exports.onPackageCreated =
    functions.firestore
    .document('packages/{packageId}')
    .onCreate((snap, context) => {
        // Get the note document
        const pack = snap.data();

        // Add an 'objectID' field which Algolia requires
        pack.objectID = context.params.packageId;

        // Write to the algolia index
        const index = client.initIndex(ALGOLIA_INDEX_NAME);
        return index.saveObject(pack);
    });
