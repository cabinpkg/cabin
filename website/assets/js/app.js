import "./phx";
import "./style";

import firebase from "firebase";
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
    // Create a reference to the cities collection
    db.collection("tokens")
    // Create a query against the collection.
        .where("owner", "==", "matken11235") // TODO;
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
        });
});

app.ports.createToken.subscribe(function(newTokenName) {
    db.collection("tokens").add({
        name: newTokenName,
        owner: "matken11235", // TODO:
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
