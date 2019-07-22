import * as admin from "firebase-admin";
import * as functions from 'firebase-functions';

admin.initializeApp(functions.config().firebase);
const firestore = admin.firestore();

export { admin, firestore, functions };
