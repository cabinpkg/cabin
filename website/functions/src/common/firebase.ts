import * as admin from "firebase-admin";
import * as functions from 'firebase-functions';

admin.initializeApp(functions.config().firebase);
const firestore = admin.firestore();
firestore.settings({
    timestampsInSnapshots: true
});

export { admin, firestore, functions };
