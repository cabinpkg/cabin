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



var user = null;


firebase.auth().onAuthStateChanged((user) => {
    if (user) {
        // サインイン済み
        var displayName = "";
        user.providerData.forEach(function (profile) {
            displayName = profile.displayName;
        }); // TODO: 複数ユーザーの情報に未対応??
        db.collection("users")
            .where("name", "==", displayName)
            .get()
            .then(function(querySnapshot) {
                querySnapshot.forEach(function(doc) {
                    var userInfo = doc.data();
                    userInfo.id = doc.id;
                    app.ports.getAuth.send(userInfo);
                });
            }).catch(function(error) {
                // console.log("Error getting document:", error);
                // app.ports.recieveUser.send(null);
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
    db.collection("users").doc(userId).get()
        .then(function(doc) {
            if (doc.exists) {
                // console.log("Document data:", doc.data());
                var userInfo = doc.data();
                userInfo.id = doc.id;
                app.ports.recieveUser.send(userInfo);
            } else {
                // doc.data() will be undefined in this case
                // console.log("No such document!");
                app.ports.recieveUser.send(null);
            }
        }).catch(function(error) {
            // console.log("Error getting document:", error);
            app.ports.recieveUser.send(null);
        });
});



import moment from "moment";
// 現在ログイン中のユーザーのIDを使用して，それが所有権を持つTokenを取得する．
app.ports.fetchToken.subscribe(function(id) {
    db.collection("tokens")
    // Create a query against the collection.
        .where("owner", "==", id) // TODO;
        .get()
        .then(function(querySnapshot) {
            var list = [];
            querySnapshot.forEach(function(doc) {
                // doc.data() is never undefined for query doc snapshots
                var token = doc.data();
                token["id"] = doc.id;
                token["created_date"] = moment(token["created_date"]).format("YYYY-MM-DD HH:mm:ss");
                list.push(token);
            });
            app.ports.recieveToken.send(list);
        }); // TODO: catch => null
});

app.ports.createToken.subscribe(function(newTokenName) {
    db.collection("tokens").add({
        name: newTokenName,
        owner: user.id, // TODO: !!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!
        created_date: Date.now(),
        last_used_date: null
    })
    .then(function(docRef) {
        // console.log("Document written with ID: ", docRef.id);
    })
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
