import "./style";

import firebase from "firebase/app";
import "firebase/auth";
import "firebase/firestore";
import "firebase/storage";

// Initialize Firebase
var config = {
    apiKey: "AIzaSyBx9dmh29ijScaFd654_LRFUX1TrSDHyPQ",
    authDomain: "poac-pm.firebaseapp.com",
    databaseURL: "https://poac-pm.firebaseio.com",
    projectId: "poac-pm",
    storageBucket: "re.poac.pm"
};
firebase.initializeApp(config);
// Initialize Cloud Firestore through Firebase
var db = firebase.firestore();
db.settings({
    timestampsInSnapshots: true
});

// Get a reference to the storage service,
//  which is used to create references in your storage bucket
var storage = firebase.storage();
// Create a storage reference from our storage service
var storageRef = storage.ref();


import Elm from "../elm/Main.elm";

const elmDiv = document.getElementById("elm");
const app = Elm.Main.embed(elmDiv);



// Create user (write to firestore)
firebase.auth().getRedirectResult().then(function(result) {
    // The signed-in user info.
    var userInfo = {
        "id": result.additionalUserInfo.profile.login,
        "name": result.additionalUserInfo.profile.name,
        "photo_url": result.additionalUserInfo.profile.avatar_url,
        "github_link": result.additionalUserInfo.profile.html_url
    };
    db.collection("users").doc(result.user.uid).set(userInfo);
}).catch(function(error) {
    // Handle Errors here.
    var errorCode = error.code;
    var errorMessage = error.message;
    // The email of the user's account used.
    var email = error.email;
    // The firebase.auth.AuthCredential type that was used.
    var credential = error.credential;
    // ...
});

// Local Storage の firebase auth UID から firestore の user情報を取得
firebase.auth().onAuthStateChanged(function (user) {
    if (user) {
        // サインイン済み
        console.log(user.uid);
        db.collection("users").doc(user.uid)
            .get()
            .then(function(doc) {
                if (doc.exists) {
                    app.ports.getAuth.send(doc.data());
                }
            }).catch(function(error) {
            // console.log("Error getting document:", error);
            // app.ports.getAuth.send(null);
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


app.ports.fetchUser.subscribe(function(userId) {
    db.collection("users")
        .where("id", "==", userId)
        .get()
        .then(function(querySnapshot) {
            querySnapshot.forEach(function (doc) {
                app.ports.recieveUser.send(doc.data());
            });
        }).catch(function(error) {
            // console.log("Error getting document:", error);
            app.ports.recieveUser.send(null);
        });
});



import moment from "moment";
// 現在ログイン中のユーザーのIDを使用して，それが所有権を持つTokenを取得する．
app.ports.fetchToken.subscribe(function() {
    const user = firebase.auth().currentUser;
    if (user) {
        db.collection("tokens")
        // Create a query against the collection.
            .where("owner", "==", user.uid)
            .get()
            .then(function (querySnapshot) {
                var list = [];
                querySnapshot.forEach(function (doc) {
                    // doc.data() is never undefined for query doc snapshots
                    var token = doc.data();
                    token["id"] = doc.id;
                    token["created_date"] = moment(token["created_date"]).format("YYYY-MM-DD HH:mm:ss");
                    list.push(token);
                });
                app.ports.recieveToken.send(list);
            }); // TODO: catch => null
    }
});

app.ports.createToken.subscribe(function(newTokenName) {
    const user = firebase.auth().currentUser;
    if (user) {
        db.collection("tokens").add({
            name: newTokenName,
            owner: user.uid,
            created_date: Date.now(),
            last_used_date: null
        })
        .then(function (docRef) {
            // console.log("Document written with ID: ", docRef.id);
        });
    }
});

app.ports.deleteToken.subscribe(function(id) {
    db.collection("tokens").doc(id).delete()
        .then(function() {
            // console.log("Document successfully deleted!");
        }).catch(function(error) {
            // console.error("Error removing document: ", error);
        });
});


app.ports.fetchPackages.subscribe(function() {
    // TODO: パッケージ全部のうち，ページングされた20個で，バージョンが最新のもの．
    db.collection("packages")
        .get()
        .then(function(querySnapshot) {
            var list = [];
            querySnapshot.forEach(function(doc) {
                // doc.data() is never undefined for query doc snapshots
                const pack = doc.data();
                var itibu = {
                    "name": pack["name"],
                    "version": pack["version"],
                    "owners": pack["owners"],
                    "cpp_version": pack["cpp_version"],
                    "description": pack["description"]
                };
                list.push(itibu);
            });
            app.ports.recievePackages.send(list);
        }); // TODO: catch => null
});

app.ports.fetchOwnedPackages.subscribe(function(userId) {
    db.collection("packages")
        .where("owners", "array-contains", userId)
        .get()
        .then(function(querySnapshot) {
            var list = [];
            querySnapshot.forEach(function(doc) {
                // doc.data() is never undefined for query doc snapshots
                const pack = doc.data();
                var itibu = {
                    "name": pack["name"],
                    "version": pack["version"],
                    "owners": pack["owners"],
                    "cpp_version": pack["cpp_version"],
                    "description": pack["description"]
                };
                list.push(itibu);
            });
            app.ports.recievePackages.send(list);
        });  // TODO: catch => null
});


function get_links(pack_links) {
    // 他のKeyを忍ばせてCrashさせられることへの対策
    var links;
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
        for (var key in pack_deps) {
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

app.ports.fetchDetailedPackage.subscribe(function(name) {
    db.collection("packages")
        .where("name", "==", name)
        .get()
        .then(function(querySnapshot) {
            var itibu = {};
            var versions = [];
            querySnapshot.forEach(function(doc) {
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

            if (versions.length != 0) {
                itibu["versions"] = versions;

                const object_name = name.replace("/", "-") + "-" + versions[0] + ".tar.gz";
                // Create a reference to the file whose metadata we want to retrieve
                var forestRef = storageRef.child(object_name);
                // Get metadata properties
                forestRef.getMetadata().then((metadata) => {
                    itibu["md5hash"] = metadata["md5Hash"];
                    itibu["created_date"] = metadata["timeCreated"];

                    app.ports.recieveDetailedPackage.send(itibu);
                // Metadata now contains the metadata for 'images/forest.jpg'
                }).catch(function(error) {
                    // Uh-oh, an error occurred!
                    app.ports.recieveDetailedPackage.send(null);
                });
            }
            else {
                app.ports.recieveDetailedPackage.send(null);
            }
        })
        .catch(function(reason) {
            app.ports.recieveDetailedPackage.send(null);
        });
});


var scroll = window.pageYOffset || document.body.scrollTop;
window.onscroll = function() {
    var newScroll = window.pageYOffset || document.body.scrollTop;
    app.ports.scroll.send([scroll, newScroll]);
    scroll = newScroll;
    // console.log(scroll);
};

app.ports.createGraph.subscribe(function() {
    requestAnimationFrame(function () {
        /* when this callback executes, the view should have rendered. */
        var data = {
            labels: ['Oct 12', 'Oct 13', 'Oct 14', 'Oct 15', 'Oct 16', 'Oct 17'],
            series: [
                [3, 2, 8, 5, 4, 6]
            ]
        };
        var options = {
            height: 300,
            high: 10,
            low: 0,
            showArea: true,
            showPoint: false,
            fullWidth: true
        };
        var chart = new Chartist.Line('.ct-chart', data, options);
        chart.on('draw', function(data) {
            if(data.type === 'line' || data.type === 'area') {
                data.element.animate({
                    d: {
                        begin: 2000 * data.index,
                        dur: 2000,
                        from: data.path.clone().scale(1, 0).translate(0, data.chartRect.height()).stringify(),
                        to: data.path.clone().stringify(),
                        easing: Chartist.Svg.Easing.easeOutQuint
                    }
                });
            }
        });
    });
});


var client = algoliasearch('IOCVK5FECM', '9c0a76bacf692daa9e8eca2aaff4b2ab');
var index = client.initIndex('packages');
app.ports.suggest.subscribe(function () {
    requestAnimationFrame(function () {
        //initialize autocomplete on search input (ID selector must match)
        autocomplete('#aa-search-input',
            { hint: true }, {
                source: autocomplete.sources.hits(index, {hitsPerPage: 5}),
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
