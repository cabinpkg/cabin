module Routing exposing (Route(..), matchers, parseUrl, pathFor)

import Url exposing (Url)
import Url.Parser exposing (..)


type Route
    = HomeIndexRoute
    | PackagesRoute String
    | OrgPackagesRoute String String
    | DonateRoute
    | UsersRoute String
    | SettingsRoute String
    | SettingRoute
    | NotFoundRoute


matchers : Parser (Route -> a) a
matchers =
    oneOf
        [ map HomeIndexRoute top
        , map PackagesRoute (s "packages" </> string)
        , map OrgPackagesRoute (s "packages" </> string </> string)
        , map DonateRoute (s "donate")
        , map UsersRoute (s "users" </> string)
        , map SettingsRoute (s "settings" </> string)
        , map SettingRoute (s "settings")
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

        PackagesRoute name ->
            "/packages/" ++ name

        OrgPackagesRoute org name ->
            "/packages/" ++ org ++ "/" ++ name

        DonateRoute ->
            "/donate"

        UsersRoute userId ->
            "/users/" ++ userId

        SettingsRoute menu ->
            "/settings/" ++ menu

        SettingRoute ->
            "/settings"

        NotFoundRoute ->
            "/not-found"
