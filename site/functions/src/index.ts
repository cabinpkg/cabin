import * as express from "express";
import * as bodyParser from "body-parser";
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



async function getDeps(name: string, version: string): Promise<any> {
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

    const querySnapshot = await getDeps(name, version);
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

    const querySnapshot = await getDeps(name, version);
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


async function getPackages(name: string, version: string): Promise<any> {
    return await firestore.collection("packages")
        .where("name", "==", name)
        .where("version", "==", version)
        .get();
}

// exist => true
router.get("/packages/:name/:version/exists", async (req: express.Request, res: express.Response) => {
    const name = req.params.name;
    const version = req.params.version;

    const querySnapshot = await getPackages(name, version);
    // Ref: https://stackoverflow.com/questions/14774907/typescript-convert-a-bool-to-string-value
    const restr: string = <string><any>(!querySnapshot.empty);
    res.status(200).send(restr);
});

router.get("/packages/:org/:name/:version/exists", async (req: express.Request, res: express.Response) => {
    const org = req.params.org;
    const name = org + "/" + req.params.name;
    const version = req.params.version;

    const querySnapshot = await getPackages(name, version);
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
