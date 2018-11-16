import * as express from "express";
import * as bodyParser from "body-parser";
import router from "./router";


const app = express();

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
// Mount the router on the app
app.use('/api/', router);


export default app;
