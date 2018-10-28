import * as express from "express";
import * as functions from 'firebase-functions';
import * as admin from 'firebase-admin';
admin.initializeApp(functions.config().firebase);
const firestore = admin.firestore();
firestore.settings({
    timestampsInSnapshots: true
});


const app = express();
// https://expressjs.com/en/advanced/best-practice-security.html#at-a-minimum-disable-x-powered-by-header
app.disable("x-powered-by");



async function getDeps(name: string, version: string): Promise<any> {
    return await firestore.collection("packages")
        .where("name", "==", name)
        .where("version", "==", version)
        .get();
}

app.get("/api/packages/:name/:version/deps", async (req: express.Request, res: express.Response) => {
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
            res.status(500).send("null");
        }
    });
});

app.get("/api/packages/:org/:name/:version/deps", async (req: express.Request, res: express.Response) => {
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
            res.status(500).send("null");
        }
    });
});


async function getVersions(name: string): Promise<any> {
    return await firestore.collection("packages")
        .where("name", "==", name)
        .get();
}

app.get("/api/packages/:name/versions", async (req: express.Request, res: express.Response) => {
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
        res.status(500).send("null");
    }
});

app.get("/api/packages/:org/:name/versions", async (req: express.Request, res: express.Response) => {
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
        res.status(500).send("null");
    }
});



// This line is important. What we are doing here
//  is exporting ONE function with the name
//  `api` which will always route
exports.api = functions.https.onRequest(app);
