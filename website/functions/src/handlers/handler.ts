import { client, ALGOLIA_INDEX_NAME } from "../common/algolia";
import { functions } from "../common/firebase";
import { CloudFunction } from "firebase-functions";
import { DocumentSnapshot } from "firebase-functions/lib/providers/firestore";


class Handler {
    onPackageCreated: CloudFunction<DocumentSnapshot>;

    constructor() {
        this.onPackageCreated = functions.firestore
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
    }
}
export default new Handler();
