module Routing exposing (Route(..), matchers, parseUrl, pathFor)

import Url exposing (Url)
import Url.Parser exposing (..)


type Route
    = HomeIndexRoute
    | PackagesRoute
    | PackageRoute String
    | OrgPackageRoute String String
    | PolicyRoute
    | PoliciesRoute String
    | NotFoundRoute


matchers : Parser (Route -> a) a
matchers =
    oneOf
        [ map HomeIndexRoute top
        , map PackagesRoute (s "packages")
        , map PackageRoute (s "packages" </> string)
        , map OrgPackageRoute (s "packages" </> string </> string)
        , map PolicyRoute (s "policies")
        , map PoliciesRoute (s "policies" </> string)
        ]


parseUrl : Url -> Route
parseUrl url =
    case parse matchers url of
        Just route ->
            route

        Nothing ->
            NotFoundRoute


pathFor : Route -> String
pathFor route =
    case route of
        HomeIndexRoute ->
            "/"

        PackagesRoute ->
            "/packages"
        PackageRoute name ->
            "/packages/" ++ name

        OrgPackageRoute org name ->
            "/packages/" ++ org ++ "/" ++ name

        PolicyRoute ->
            "/policies"
        PoliciesRoute name ->
            "/policies/" ++ name

        NotFoundRoute ->
            "/not-found"
