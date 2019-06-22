import "../scss/app.scss";

// Initialize Cloud Firestore through Firebase
const db = firebase.firestore();
// Get a reference to the storage service,
//  which is used to create references in your storage bucket
// Create a storage reference from our storage service
const storageRef = firebase.storage().ref();

import { Elm } from "../elm/Main.elm";
const flags = { api: "https://poac.pm" };
const app = Elm.Main.init({ flags: flags });


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
            "name": name, // TODO: 最新のを選択すべき // FIXME
            "versions": version,
            "owners": pack["owners"],
            "cpp_version": pack["cpp_version"],
            "description": pack["description"],
            "deps": get_deps(pack["deps"]),
            "links": pack["links"],
            "license": pack["license"]
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
                container: '#cpp-version',
                attributeName: 'cpp_version',
                operator: 'or',
                limit: 10,
                templates: {
                    header: 'C++ version'
                }
            })
        );
        search.addWidget(
            instantsearch.widgets.refinementList({
                container: '#package-type',
                attributeName: 'package_type',
                operator: 'or',
                limit: 10,
                templates: {
                    header: 'Package Type'
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
                pushFunction: () => {
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

// Algolia側で，C++ versionはIntegerとして設定されているため，C++03が，3と表示されてしまう．
// それを強制的に書き換える．
requestAnimationFrame(() => {
    setInterval(() => {
        let q = document.querySelector('[value="3"]');
        if (q) {
            let nq = q.nextSibling;
            if (nq) {
                nq.textContent = " 03 ";
            }
        }
    }, 1000);
});
