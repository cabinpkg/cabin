import * as express from "express";
import controller from './controller';
export default express.Router()
    .get("/packages/:name/:version/deps", controller.deps)
    .get("/packages/:org/:name/:version/deps", controller.orgDeps)
    .get("/packages/:name/versions", controller.versions)
    .get("/packages/:org/:name/versions", controller.orgVersions)
    .get("/packages/:name/:version/exists", controller.exists)
    .get("/packages/:org/:name/:version/exists", controller.orgExists)
    .post("/tokens/validate", controller.tokenValidate);
