import algoliasearch from "algoliasearch";
import { functions } from "../common/firebase";


// Initialize Algolia, requires installing Algolia dependencies:
// https://www.algolia.com/doc/api-client/javascript/getting-started/#install
//
// App ID and API Key are stored in functions config variables
const ALGOLIA_ID = functions.config().algolia.app_id;
const ALGOLIA_ADMIN_KEY = functions.config().algolia.api_key;
// const ALGOLIA_SEARCH_KEY = functions.config().algolia.search_key;

const ALGOLIA_INDEX_NAME = 'packages';
const client = algoliasearch(ALGOLIA_ID, ALGOLIA_ADMIN_KEY);


export { client, ALGOLIA_ID, ALGOLIA_ADMIN_KEY, ALGOLIA_INDEX_NAME };
