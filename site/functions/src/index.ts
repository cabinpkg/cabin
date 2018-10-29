import * as express from "express";
import * as bodyParser from "body-parser";
import * as algoliasearch from "algoliasearch";
import * as functions from 'firebase-functions';
import * as admin from 'firebase-admin';

admin.initializeApp(functions.config().firebase);
const firestore = admin.firestore();
firestore.settings({
    timestampsInSnapshots: true
});


const app = express();
const router = express.Router();

// Parse application/json
app.use(bodyParser.json({
    type: 'application/*+json'
}));
// Parse application/x-www-form-urlencoded
app.use(bodyParser.urlencoded({
    extended: false,
    type: 'application/x-www-form-urlencoded'
}));

// https://expressjs.com/en/advanced/best-practice-security.html#at-a-minimum-disable-x-powered-by-header
app.disable("x-powered-by");



async function getSpecNameVersion(name: string, version: string): Promise<any> {
    return await firestore.collection("packages")
        .where("name", "==", name)
        .where("version", "==", version)
        .get();
}

router.get("/packages/:name/:version/deps", async (req: express.Request, res: express.Response) => {
    // Guess what, uid will NEVER be null in this
    //  context because of the Express router.
    const name = req.params.name;
    const version = req.params.version;

    const querySnapshot = await getSpecNameVersion(name, version);
    querySnapshot.forEach((doc) => {
        const packageInfo = doc.data();
        const deps = packageInfo.deps;
        if (deps) {
            res.status(200).send(deps);
        } else {
            res.status(200).send("null");
        }
    });
});

router.get("/packages/:org/:name/:version/deps", async (req: express.Request, res: express.Response) => {
    const org = req.params.org;
    const name = org + "/" + req.params.name;
    const version = req.params.version;

    const querySnapshot = await getSpecNameVersion(name, version);
    querySnapshot.forEach((doc) => {
        const packageInfo = doc.data();
        const deps = packageInfo.deps;
        if (deps) {
            res.status(200).send(deps);
        } else {
            res.status(200).send("null");
        }
    });
});


async function getVersions(name: string): Promise<any> {
    return await firestore.collection("packages")
        .where("name", "==", name)
        .get();
}

router.get("/packages/:name/versions", async (req: express.Request, res: express.Response) => {
    // Guess what, uid will NEVER be null in this
    //  context because of the Express router.
    const name = req.params.name;

    const querySnapshot = await getVersions(name);
    const versions = [];
    querySnapshot.forEach((doc) => {
        const packageInfo = doc.data();
        versions.push(packageInfo.version);
    });

    if (versions.length !== 0) {
        res.status(200).send(versions);
    } else {
        res.status(200).send("null");
    }
});

router.get("/packages/:org/:name/versions", async (req: express.Request, res: express.Response) => {
    const org = req.params.org;
    const name = org + "/" + req.params.name;

    const querySnapshot = await getVersions(name);
    const versions = [];
    querySnapshot.forEach((doc) => {
        const packageInfo = doc.data();
        versions.push(packageInfo.version);
    });

    if (versions.length !== 0) {
        res.status(200).send(versions);
    } else {
        res.status(200).send("null");
    }
});


// exist => true
router.get("/packages/:name/:version/exists", async (req: express.Request, res: express.Response) => {
    const name = req.params.name;
    const version = req.params.version;

    const querySnapshot = await getSpecNameVersion(name, version);
    // Ref: https://stackoverflow.com/questions/14774907/typescript-convert-a-bool-to-string-value
    const restr: string = <string><any>(!querySnapshot.empty);
    res.status(200).send(restr);
});

router.get("/packages/:org/:name/:version/exists", async (req: express.Request, res: express.Response) => {
    const org = req.params.org;
    const name = org + "/" + req.params.name;
    const version = req.params.version;

    const querySnapshot = await getSpecNameVersion(name, version);
    const restr: string = <string><any>(!querySnapshot.empty);
    res.status(200).send(restr);
});


async function getToken(token: string): Promise<any> {
    return await firestore.collection("tokens")
        .where(admin.firestore.FieldPath.documentId(), "==", token)
        .get();
}

router.post("/tokens/validate", async (req: express.Request, res: express.Response) => {
    const owners: Array<string> = req.body.owners;
    const querySnapshot = await getToken(req.body.token);
    if (querySnapshot.empty) {
        res.status(200).send("err");
    } else {
        let own: boolean = false;
        querySnapshot.forEach((doc) => {
            const tokenInfo = doc.data();
            own = own || (owners.indexOf(tokenInfo.owner) > -1);
        });

        if (own) {
            res.status(200).send("ok");
        } else {
            res.status(200).send("err");
        }
    }
});



// Mount the router on the app
app.use('/api/', router);
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
exports.onPackageCreated = functions.firestore
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
