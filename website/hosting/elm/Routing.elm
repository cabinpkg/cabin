module Routing exposing (..)

import Navigation
import UrlParser exposing (..)


type Route
    = HomeIndexRoute
    | SearchRoute (Maybe String)
    | PackagesRoute String
    | OrgPackagesRoute String String
    | DonateRoute
    | UsersRoute String
    | SettingsRoute String
    | SettingRoute
    | NotFoundRoute


toPath : Route -> String
toPath route =
    case route of
        HomeIndexRoute ->
            "/"

        SearchRoute (Just word) ->
            "/search?q=" ++ word
        SearchRoute Nothing ->
            "/search/"

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


matchers : Parser (Route -> a) a
matchers =
    oneOf
        [ map HomeIndexRoute top
        , map SearchRoute <| s "search" <?> stringParam "q"
        , map PackagesRoute <| s "packages" </> string
        , map OrgPackagesRoute <| s "packages" </> string </> string
        , map DonateRoute <| s "donate"
        , map UsersRoute <| s "users" </> string
        , map SettingsRoute <| s "settings" </> string
        , map SettingRoute <| s "settings"
        ]


parse : Navigation.Location -> Route
parse location =
    case UrlParser.parsePath matchers location of
        Just route ->
            route

        Nothing ->
            NotFoundRoute
