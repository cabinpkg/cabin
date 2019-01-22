import { client, ALGOLIA_INDEX_NAME } from "../common/algolia";
import { functions, storage } from "../common/firebase";
import { CloudFunction } from "firebase-functions";
import { DocumentSnapshot } from "firebase-functions/lib/providers/firestore";


class Handler {
    onMetadataCreated: CloudFunction<DocumentSnapshot>;
    onMetadataDeleted: CloudFunction<DocumentSnapshot>;

    constructor() {
        this.onMetadataCreated = functions.firestore
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
        this.onMetadataDeleted = functions.firestore
            .document('packages/{packageId}')
            .onDelete((snap, context) => {
                const index = client.initIndex(ALGOLIA_INDEX_NAME);
                index.deleteObject(snap.id)
                    .then(() => {
                        const name = snap.get('name');
                        const version = snap.get('version');
                        const bucketName = name + "-" + version + ".tar.gz";
                        return storage.bucket().file(bucketName).delete();
                    })
                    .catch((err) => {
                        console.error(err);
                        return null;
                    });
            });
    }
}
export default new Handler();
