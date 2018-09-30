module Routing exposing (..)

import Navigation
import UrlParser exposing (..)


type Route
    = HomeIndexRoute
    | PackagesRoute String
    | DonateRoute
    | UsersRoute String
    | SettingsRoute String
    | SettingRoute
    | NotFoundRoute


-- toPath is unnecessary?
toPath : Route -> String
toPath route =
    case route of
        HomeIndexRoute ->
            "/"

        PackagesRoute name ->
            "/packages/" ++ name

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
        , map PackagesRoute <| s "packages" </> string
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
