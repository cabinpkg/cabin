import "../scss/pc.scss";
import "../scss/mobile.scss";


// Initialize Cloud Firestore through Firebase
const db = firebase.firestore();
db.settings({
    timestampsInSnapshots: true
});

// Get a reference to the storage service,
//  which is used to create references in your storage bucket
// Create a storage reference from our storage service
const storageRef = firebase.storage().ref();


import { Elm } from "../elm/Main.elm";
const flags = { api: "https://poac.pm" };
const app = Elm.Main.init({ flags: flags });



function getCurrentUser() {
    return firebase.auth().currentUser;
}

firebase.auth().onAuthStateChanged(async (user) => {
    if (user) {
        app.ports.receiveSigninUser.send({
            "name": user.displayName,
            "photo_url": user.photoURL
        });

        // Get document id
        const doc = await db.collection("users").doc(user.uid).get().catch(() => {
            app.ports.receiveSigninId.send(null);
        });
        app.ports.receiveSigninId.send(doc.data().id);

        // Create user (write to firestore)
        const redirect_result = await firebase.auth().getRedirectResult();
        if (redirect_result.user !== null) {
            // The signed-in user info.
            const userInfo = {
                "id": redirect_result.additionalUserInfo.profile.login,
                "name": redirect_result.additionalUserInfo.profile.name,
                "photo_url": redirect_result.additionalUserInfo.profile.avatar_url,
                "github_link": redirect_result.additionalUserInfo.profile.html_url
            };
            db.collection("users").doc(redirect_result.user.uid).set(userInfo);
        }
    }
});

app.ports.signin.subscribe(async () => {
    const provider = new firebase.auth.GithubAuthProvider();
    provider.addScope('public_repo,read:org');
    await firebase.auth().setPersistence(firebase.auth.Auth.Persistence.LOCAL);
    await firebase.auth().signInWithRedirect(provider);
});
app.ports.signout.subscribe(async () => {
    await firebase.auth().setPersistence(firebase.auth.Auth.Persistence.NONE);
});


app.ports.fetchUser.subscribe(async (userId) => {
    const querySnapshot = await db.collection("users").where("id", "==", userId).get().catch(() => {
        app.ports.receiveUser.send(null);
    });
    if (querySnapshot.empty) {
        app.ports.receiveUser.send(null);
    }
    else {
        querySnapshot.forEach(function (doc) {
            app.ports.receiveUser.send(doc.data());
        });
    }
});


// 現在ログイン中のユーザーのIDを使用して，それが所有権を持つTokenを取得する．
app.ports.fetchToken.subscribe(async () => {
    const user = getCurrentUser();
    if (user) {
        const querySnapshot = await db.collection("tokens").where("owner", "==", user.uid).get();
        let list = [];
        querySnapshot.forEach((doc) => {
            // doc.data() is never undefined for query doc snapshots
            let token = doc.data();
            token["id"] = doc.id;
            token["created_date"] = moment(token["created_date"]).format("YYYY-MM-DD HH:mm:ss");
            list.push(token);
        });
        app.ports.receiveToken.send(list);
    }
});

app.ports.createToken.subscribe(async (newTokenName) => {
    const user = getCurrentUser();
    if (user) {
        await db.collection("tokens").add({
            name: newTokenName,
            owner: user.uid,
            created_date: Date.now(),
            last_used_date: null
        });
    }
});

app.ports.deleteToken.subscribe(async (id) => {
    await db.collection("tokens").doc(id).delete();
});


app.ports.fetchSigninUserId.subscribe(async () => { // TODO: 名称が分かりづらすぎる
    const user = getCurrentUser();
    if (user) {
        const doc = await db.collection("users").doc(user.uid).get();
        const userId = doc.data()["id"];

        const querySnapshot = await db.collection("packages").where("owners", "array-contains", userId).get();
        let list = [];
        querySnapshot.forEach((doc) => {
            // doc.data() is never undefined for query doc snapshots
            const pack = doc.data();
            const itibu = {
                "name": pack["name"],
                "version": pack["version"],
                "owners": pack["owners"],
                "cpp_version": pack["cpp_version"],
                "description": pack["description"]
            };
            list.push(itibu);
        });
        app.ports.receivePackages.send(list);
    }
});


app.ports.fetchOwnedPackages.subscribe(async (userId) => {
    const querySnapshot = await db.collection("packages").where("owners", "array-contains", userId).get();
    let list = [];
    querySnapshot.forEach((doc) => {
        // doc.data() is never undefined for query doc snapshots
        const pack = doc.data();
        const itibu = {
            "name": pack["name"],
            "version": pack["version"],
            "owners": pack["owners"],
            "cpp_version": pack["cpp_version"],
            "description": pack["description"]
        };
        list.push(itibu);
    });
    app.ports.receivePackages.send(list);
});

app.ports.deletePackage.subscribe(async (argv) => {
    let confirmStr = "This action cannot be undone. Are you sure you want to permanently delete ";
    confirmStr += argv[0] + ": " + argv[1] + "?";

    if (window.confirm(confirmStr)) {
        const querySnapshot = await db.collection("packages").where("name", "==", argv[0]).where("version", "==", argv[1]).get();
        querySnapshot.forEach((doc) => {
            doc.ref.delete();
        });
        location.reload();
    }
});


function get_links(pack_links) {
    // 他のKeyを忍ばせてCrashさせられることへの対策
    let links;
    if (pack_links == null) {
        links = null;
    } else {
        links = {};
        if (pack_links["github"] == null) {
            links["github"] = null;
        } else {
            links["github"] = pack_links["github"];
        }
        if (pack_links["homepage"] == null) {
            links["homepage"] = null;
        } else {
            links["homepage"] = pack_links["homepage"];
        }
    }
    return links;
}

function get_deps(pack_deps) {
    let deps;
    if (pack_deps == null) {
        deps = null;
    } else {
        deps = [];
        for (let key in pack_deps) {
            // src == poac
            if ((typeof pack_deps[key]) == "string") {
                const dep = {
                    "name": key,
                    "version": pack_deps[key]
                };
                deps.push(dep);
            }
            else if (pack_deps[key]["src"] === "poac") {
                const dep = {
                    "name": key,
                    "version": pack_deps[key]["version"]
                };
                deps.push(dep);
            }
            else if (pack_deps[key]["src"] === "github") {
                const dep = {
                    "name": key,
                    "version": pack_deps[key]["tag"]
                };
                deps.push(dep);
            }
        }
    }
    // console.log(deps);
    return deps;
}

app.ports.fetchDetailedPackage.subscribe(async (name) => {
    const querySnapshot = await db.collection("packages").where("name", "==", name).get().catch(() => {
        app.ports.receiveDetailedPackage.send(null);
    });

    let itibu = {};
    let versions = [];
    querySnapshot.forEach((doc) => {
        // doc.data() is never undefined for query doc snapshots
        const pack = doc.data();
        const name = pack["name"];
        const version = pack["version"];

        itibu = {
            "name": name, // TODO: 最新のを選択すべき
            "versions": version,
            "owners": pack["owners"],
            "cpp_version": pack["cpp_version"],
            "description": pack["description"],
            "deps": get_deps(pack["deps"]),
            "links": get_links(pack["links"]),
            "license": pack["license"] == null ? null : pack["license"]
        };
        versions.push(pack["version"]);
    });

    if (!versions.empty) {
        itibu["versions"] = versions;

        const package_name = name.replace("/", "-") + "-" + versions[0];
        const object_name = package_name + ".tar.gz";
        // Create a reference to the file whose metadata we want to retrieve
        const packageRef = storageRef.child(object_name);
        // Get metadata properties
        const metadata = await packageRef.getMetadata().catch(() => {
            app.ports.receiveDetailedPackage.send(null);
        });

        itibu["md5hash"] = metadata["md5Hash"];
        itibu["created_date"] = metadata["timeCreated"];
        app.ports.receiveDetailedPackage.send(itibu);
    }
    else {
        app.ports.receiveDetailedPackage.send(null);
    }
});


let scroll = window.pageYOffset || document.body.scrollTop;
window.onscroll = () => {
    const newScroll = window.pageYOffset || document.body.scrollTop;
    app.ports.onScroll.send(newScroll);
    scroll = newScroll;
    // console.log(scroll);
};


app.ports.onWidth.send(window.innerWidth);
window.onresize = () => {
    app.ports.onWidth.send(window.innerWidth);
    // console.log(window.innerWidth);
};


const client = algoliasearch('IOCVK5FECM', '9c0a76bacf692daa9e8eca2aaff4b2ab');
const index = client.initIndex('packages');
app.ports.suggest.subscribe(() => {
    requestAnimationFrame(() => {
        //initialize autocomplete on search input (ID selector must match)
        autocomplete('#aa-search-input',
            { hint: true }, {
                source: autocomplete.sources.hits(index, {hitsPerPage: 5, distinct: true}),
                //value to be displayed in input control after user's suggestion selection
                displayKey: 'name',
                //hash of templates used when rendering dataset
                templates: {
                    //'suggestion' templating function used to render a single suggestion
                    suggestion: function(suggestion) {
                        return '<span>' +
                            suggestion._highlightResult.name.value + '</span>';
                    }
                }
            });
    });
});


const search = instantsearch({
    // Replace with your own values
    appId: 'IOCVK5FECM',
    apiKey: '9c0a76bacf692daa9e8eca2aaff4b2ab', // search only API key, no ADMIN key
    indexName: 'packages',
    urlSync: true, // This will keep the browser url in sync and allow users to copy paste urls corresponding to the current search state.
    searchParameters: {
        hitsPerPage: 20,
        distinct: true
    }
});
let isSearchable = false;
app.ports.instantsearch.subscribe(() => {
    requestAnimationFrame(() => {
        search.addWidget(
            instantsearch.widgets.searchBox({
                container: '#search-input'
            })
        );
        search.addWidget(
            instantsearch.widgets.stats({
                container: '#search-top-container',
                templates: {
                    body: "{{nbHits}} packages found"
                }
            })
        );
        search.addWidget(
            instantsearch.widgets.refinementList({
                container: '#search-refinement-list',
                attributeName: 'cpp_version',
                operator: 'or',
                limit: 10,
                templates: {
                    header: 'C++ version'
                }
            })
        );
        search.addWidget(
            instantsearch.widgets.hits({
                container: '#hits',
                templates: {
                    item: document.getElementById('hit-template').innerHTML,
                    empty: "We didn't find any results for the search <em>\"{{query}}\"</em>"
                }
            })
        );
        search.addWidget(
            instantsearch.widgets.pagination({
                container: '#pagination'
            })
        );
        search.addWidget(
            instantsearch.widgets.analytics({
                pushFunction: (formattedParameters, state, results) => {
                    // Google Analytics
                    window.ga('set', 'page', window.location.pathname + window.location.search);
                    window.ga('send', 'pageView');
                }
            })
        );

        if (!isSearchable) {
            // Add this after all the search.addWidget() calls
            search.start();
            // Call start only once
            isSearchable = true;
        }
    });
});
