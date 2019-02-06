import "../scss/pc.scss";
import "../scss/mobile.scss";


// Initialize Cloud Firestore through Firebase
const db = firebase.firestore();
db.settings({
    timestampsInSnapshots: true
});

// Get a reference to the storage service,
//  which is used to create references in your storage bucket
const storage = firebase.storage();
// Create a storage reference from our storage service
const storageRef = storage.ref();


import { Elm } from "../elm/Main.elm";

const flags = { api: "https://poac.io" };
const app = Elm.Main.init({ flags: flags });


firebase.auth().onAuthStateChanged((user) => {
    if (user) {
        app.ports.receiveSigninUser.send({
            "name": user.displayName,
            "photo_url": user.photoURL
        });
        db.collection("users").doc(user.uid)
            .get()
            .then((doc) => {
                app.ports.receiveSigninId.send(doc.data().id);
            }).catch((error) => {
                app.ports.receiveSigninId.send(null);
            });

        // Create user (write to firestore)
        firebase.auth().getRedirectResult().then((result) => {
            if (result.user !== null) {
                // The signed-in user info.
                const userInfo = {
                    "id": result.additionalUserInfo.profile.login,
                    "name": result.additionalUserInfo.profile.name,
                    "photo_url": result.additionalUserInfo.profile.avatar_url,
                    "github_link": result.additionalUserInfo.profile.html_url
                };
                db.collection("users").doc(result.user.uid).set(userInfo);
            }
        });
    }
});

app.ports.signin.subscribe(() => {
    const provider = new firebase.auth.GithubAuthProvider();
    provider.addScope('public_repo,read:org');
    firebase.auth().setPersistence(firebase.auth.Auth.Persistence.LOCAL)
        .then(function() {
            firebase.auth().signInWithRedirect(provider);
        });
});
app.ports.signout.subscribe(() => {
    firebase.auth().setPersistence(firebase.auth.Auth.Persistence.NONE);
});


app.ports.fetchUser.subscribe((userId) => {
    db.collection("users")
        .where("id", "==", userId)
        .get()
        .then((querySnapshot) => {
            if (querySnapshot.empty) {
                app.ports.receiveUser.send(null);
            }
            else {
                querySnapshot.forEach(function (doc) {
                    app.ports.receiveUser.send(doc.data());
                });
            }
        }).catch(function(error) {
            // console.log("Error getting document:", error);
            app.ports.receiveUser.send(null);
        });
});



// 現在ログイン中のユーザーのIDを使用して，それが所有権を持つTokenを取得する．
app.ports.fetchToken.subscribe(() => {
    const user = firebase.auth().currentUser;
    if (user) {
        db.collection("tokens")
        // Create a query against the collection.
            .where("owner", "==", user.uid)
            .get()
            .then(function (querySnapshot) {
                let list = [];
                querySnapshot.forEach((doc) => {
                    // doc.data() is never undefined for query doc snapshots
                    let token = doc.data();
                    token["id"] = doc.id;
                    token["created_date"] = moment(token["created_date"]).format("YYYY-MM-DD HH:mm:ss");
                    list.push(token);
                });
                app.ports.receiveToken.send(list);
            }); // TODO: catch => null
    }
});

app.ports.createToken.subscribe((newTokenName) => {
    const user = firebase.auth().currentUser;
    if (user) {
        db.collection("tokens").add({
            name: newTokenName,
            owner: user.uid,
            created_date: Date.now(),
            last_used_date: null
        })
        .then((docRef) => {
            // console.log("Document written with ID: ", docRef.id);
        });
    }
});

app.ports.deleteToken.subscribe((id) => {
    db.collection("tokens").doc(id).delete()
        .then(() => {
            // console.log("Document successfully deleted!");
        }).catch((error) => {
            // console.error("Error removing document: ", error);
        });
});


app.ports.fetchSigninUserId.subscribe(() => {
    const user = firebase.auth().currentUser;
    if (user) {
        db.collection("users").doc(user.uid)
            .get().then(function(doc) {
                const userId = doc.data()["id"];
                db.collection("packages")
                    .where("owners", "array-contains", userId)
                    .get()
                    .then((querySnapshot) => {
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
                    });  // TODO: catch => null
            });
    }
});


app.ports.fetchPackages.subscribe(() => {
    // TODO: パッケージ全部のうち，ページングされた20個で，バージョンが最新のもの．
    db.collection("packages")
        .get()
        .then((querySnapshot) => {
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
        }); // TODO: catch => null
});

app.ports.fetchOwnedPackages.subscribe((userId) => {
    db.collection("packages")
        .where("owners", "array-contains", userId)
        .get()
        .then((querySnapshot) => {
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
        });  // TODO: catch => null
});

app.ports.deletePackage.subscribe((argv) => {
    let confirmStr = "This action cannot be undone. Are you sure you want to permanently delete ";
    confirmStr += argv[0] + ": " + argv[1] + "?";

    if (window.confirm(confirmStr)) {
        db.collection("packages")
            .where("name", "==", argv[0])
            .where("version", "==", argv[1])
            .get()
            .then((querySnapshot) => {
                querySnapshot.forEach((doc) => {
                    doc.ref.delete();
                });
                location.reload();
            });
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
    var deps;
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
            else if (pack_deps[key]["src"] == "poac") {
                const dep = {
                    "name": key,
                    "version": pack_deps[key]["version"]
                };
                deps.push(dep);
            }
            else if (pack_deps[key]["src"] == "github") {
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

app.ports.fetchDetailedPackage.subscribe((name) => { // TODO: ここで,
    db.collection("packages")
        .where("name", "==", name)
        .get()
        .then((querySnapshot) => {
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
                const forestRef = storageRef.child(object_name);
                // Get metadata properties
                forestRef.getMetadata().then((metadata) => {
                    itibu["md5hash"] = metadata["md5Hash"];
                    itibu["created_date"] = metadata["timeCreated"];

                    app.ports.receiveDetailedPackage.send(itibu);
                // Metadata now contains the metadata for 'images/forest.jpg'
                }).catch((error) => {
                    // Uh-oh, an error occurred!
                    app.ports.receiveDetailedPackage.send(null);
                });

                // Get README.md
                storageRef.child(package_name + "/README.md").getDownloadURL().then((url) => {
                    // `url` is the download URL for 'images/stars.jpg'

                    // This can be downloaded directly:
                    var xhr = new XMLHttpRequest();
                    xhr.onreadystatechange = () => {
                        switch ( xhr.readyState ) {
                            case 0:
                                // 未初期化状態.
                                // console.log( 'uninitialized!' );
                                break;
                            case 1: // データ送信中.
                                // console.log( 'loading...' );
                                break;
                            case 2: // 応答待ち.
                                // console.log( 'loaded.' );
                                break;
                            case 3: // データ受信中.
                                // console.log( 'interactive... '+xhr.responseText.length+' bytes.' );
                                break;
                            case 4: // データ受信完了.
                                if( xhr.status == 200 || xhr.status == 304 ) {
                                    // console.log(xhr.responseText);
                                    app.ports.receiveReadme.send(xhr.responseText);
                                } else {
                                    app.ports.receiveReadme.send(null);
                                }
                                break;
                        }
                    };
                    xhr.open('GET', url);
                    xhr.send();
                }).catch(function(error) {
                    // Handle any errors
                    // TODO: 404の時にconsoleにエラーが表示されてしまう．必要がない．
                });
            }
            else {
                app.ports.receiveDetailedPackage.send(null);
            }
        })
        .catch((reason) => {
            app.ports.receiveDetailedPackage.send(null);
        });
});


let scroll = window.pageYOffset || document.body.scrollTop;
window.onscroll = () => {
    const newScroll = window.pageYOffset || document.body.scrollTop;
    app.ports.onScroll.send(newScroll);
    scroll = newScroll;
    // console.log(scroll);
};


// Initialize
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
        hitsPerPage: 10,
        distinct: true
    }
});
let isSearchable = false;
app.ports.instantsearch.subscribe(() => {
    requestAnimationFrame(() => {
        // Add this after the previous JavaScript code
        search.addWidget(
            instantsearch.widgets.searchBox({
                container: '#search-input'
            })
        );
        // Add this after the previous JavaScript code
        search.addWidget(
            instantsearch.widgets.hits({
                container: '#hits',
                templates: {
                    header: "<div class=\"search-top-container\">\"{{nbHits}} packages found\"</div>",
                    item: document.getElementById('hit-template').innerHTML,
                    empty: "We didn't find any results for the search <em>\"{{query}}\"</em>"
                }
            })
        );
        // Add this after the other search.addWidget() calls
        search.addWidget(
            instantsearch.widgets.pagination({
                container: '#pagination'
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
