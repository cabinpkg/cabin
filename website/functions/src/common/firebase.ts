import * as admin from "firebase-admin";
import * as functions from 'firebase-functions';

admin.initializeApp(functions.config().firebase);
const firestore = admin.firestore();
firestore.settings({
    timestampsInSnapshots: true
});
const storage = admin.storage();

export { admin, firestore, functions, storage };
