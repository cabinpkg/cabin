import { Request, Response } from 'express';
import { admin, firestore } from "../../common/firebase";


async function getSpecNameVersionFromFirestore(name: string, version: string): Promise<any> {
    return await firestore.collection("packages")
        .where("name", "==", name)
        .where("version", "==", version)
        .get();
}

function getQuerySnapshotHead(qs: FirebaseFirestore.QuerySnapshot)
    : FirebaseFirestore.QueryDocumentSnapshot | null {
    return qs.empty ? null : qs.docs[0];
}

function pickSpecNameVersion(qs: FirebaseFirestore.QuerySnapshot): object | null {
    const doc: FirebaseFirestore.QueryDocumentSnapshot | null = getQuerySnapshotHead(qs);

    if (doc === null) {
        return null;
    }

    const packageInfo = doc.data();
    if (packageInfo.deps) {
        let deps = packageInfo.deps;
        let dep = {};
        let i: number = 0;
        for (const key in deps) {
            if (deps[key]["src"] === "poac") {
                deps[key] = deps[key]["version"];
            }
            else if (deps[key]["src"] === "github") {
                deps[key] = deps[key]["tag"];
            }
            dep[i.toString()] = {
                "name": key,
                "version": deps[key]
            };
            ++i;
        }
        return dep;
    }
    else {
        return null;
    }
}

async function getVersions(name: string): Promise<any> {
    return await firestore.collection("packages")
        .where("name", "==", name)
        .get();
}

async function getToken(token: string): Promise<any> {
    return await firestore.collection("tokens")
        .where(admin.firestore.FieldPath.documentId(), "==", token)
        .get();
}

async function getUser(id: string): Promise<any> {
    return await firestore.collection("users")
        .where("id", "==", id)
        .get();
}

class Controller {
    async deps(req: Request, res: Response): Promise<any> {
        const name = req.params.name;
        const version = req.params.version;

        const querySnapshot: FirebaseFirestore.QuerySnapshot =
            await getSpecNameVersionFromFirestore(name, version);
        const dependency: object | null =
            pickSpecNameVersion(querySnapshot);

        res.status(200).send(dependency === null ? "null" : dependency);
    }

    async orgDeps(req: Request, res: Response): Promise<any> {
        const org = req.params.org;
        const name = org + "/" + req.params.name;
        const version = req.params.version;

        const querySnapshot: FirebaseFirestore.QuerySnapshot =
            await getSpecNameVersionFromFirestore(name, version);
        const dependency: object | null =
            pickSpecNameVersion(querySnapshot);

        res.status(200).send(dependency === null ? "null" : dependency);
    }

    async versions(req: Request, res: Response): Promise<any> {
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
    }
    async orgVersions(req: Request, res: Response): Promise<any> {
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
    }

    // exist => true
    async exists(req: Request, res: Response): Promise<any> {
        const name = req.params.name;
        const version = req.params.version;

        const querySnapshot = await getSpecNameVersionFromFirestore(name, version);
        // Ref: https://stackoverflow.com/questions/14774907/typescript-convert-a-bool-to-string-value
        const restr: string = <string><any>(!querySnapshot.empty);
        res.status(200).send(restr);
    }
    async orgExists(req: Request, res: Response): Promise<any> {
        const org = req.params.org;
        const name = org + "/" + req.params.name;
        const version = req.params.version;

        const querySnapshot = await getSpecNameVersionFromFirestore(name, version);
        const restr: string = <string><any>(!querySnapshot.empty);
        res.status(200).send(restr);
    }

    async tokenValidate(req: Request, res: Response): Promise<any> {
        const owners: Array<string> = req.body.owners;
        const querySnapshot: FirebaseFirestore.QuerySnapshot = await getToken(req.body.token);
        let querySnapshot2: Array<FirebaseFirestore.QuerySnapshot> = [];

        for (const val of owners) {
            querySnapshot2.push(await getUser(val));
        }
        let isEmpty: boolean = false;
        let users = [];
        for (const val of querySnapshot2) {
            if (val.empty) {
                isEmpty = isEmpty || val.empty;
            }
            else {
                val.forEach((doc) => {
                    users.push(doc.id);
                });
            }
        }

        if (querySnapshot.empty || isEmpty) {
            res.status(200).send("err");
        } else {
            let own: boolean = false;

            querySnapshot.forEach((doc) => {
                const tokenInfo = doc.data();
                own = own || (users.indexOf(tokenInfo.owner) > -1);
            });

            if (own) {
                res.status(200).send("ok");
            } else {
                res.status(200).send("err");
            }
        }
    }
}
export default new Controller();
