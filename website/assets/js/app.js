import "./phx";
import "./style";

import firebase from "firebase/app";
import "firebase/auth";
import "firebase/firestore";
// Initialize Firebase
var config = {
    apiKey: "AIzaSyBx9dmh29ijScaFd654_LRFUX1TrSDHyPQ",
    authDomain: "poac-pm.firebaseapp.com",
    databaseURL: "https://poac-pm.firebaseio.com",
    projectId: "poac-pm",
    storageBucket: "poac-pm.appspot.com",
    messagingSenderId: "1061286038669"
};
firebase.initializeApp(config);
// Initialize Cloud Firestore through Firebase
var db = firebase.firestore();
db.settings({
    timestampsInSnapshots: true
});


import Elm from "../elm/Main.elm";

const elmDiv = document.getElementById("elm");
const app = Elm.Main.embed(elmDiv);

app.ports.logout.subscribe(() => {
    firebase.auth().signOut();
});


var user = null;


app.ports.login.subscribe(function() {
    var provider = new firebase.auth.GithubAuthProvider();
    provider.addScope('public_repo,read:org');
    // firebase.auth().signInWithRedirect(provider);

    firebase.auth().signInWithPopup(provider).then(function(result) {
        if (result.credential) {
            // This gives you a GitHub Access Token. You can use it to access the GitHub API.
            var token = result.credential.accessToken;
            // ...
        }
        // The signed-in user info.
        // var user = result.user;
        const userId = result.additionalUserInfo.profile.login;
        var userInfo = {
            "name": result.additionalUserInfo.profile.name,
            "photo_url": result.additionalUserInfo.profile.avatar_url,
            "github_link": result.additionalUserInfo.profile.html_url
        };
        db.collection("users").doc(userId).set(userInfo);

        userInfo.id = userId;
        // console.log(userInfo);
        user = userInfo;

        app.ports.getAuth.send(userInfo);
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
app.ports.fetchToken.subscribe(function() {
    db.collection("tokens")
    // Create a query against the collection.
        .where("owner", "==", user.id) // TODO;
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
        owner: user.id, // TODO:
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

                itibu = {
                    "name": pack["name"], // TODO: 最新のを選択すべき
                    "versions": pack["version"],
                    "owners": pack["owners"],
                    "cpp_version": pack["cpp_version"],
                    "description": pack["description"],
                    "deps": get_deps(pack["deps"]),
                    "md5hash": pack["md5hash"],
                    "links": get_links(pack["links"]),
                    "license": pack["license"] == null ? null : pack["license"]
                };
                versions.push(pack["version"]);
            });
            if (versions.length != 0) {
                itibu["versions"] = versions;
                app.ports.recieveDetailedPackage.send(itibu);
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
