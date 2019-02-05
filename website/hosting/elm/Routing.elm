module Routing exposing (Route(..), matchers, parse, toPath)

import Url
import Url.Parser as Parser exposing ((</>))


type Route
    = HomeIndexRoute
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


--matchers : Parser (Route -> a) a
matchers =
    Parser.oneOf
        [ Parser.map HomeIndexRoute Parser.top
        , Parser.map PackagesRoute <| Parser.s "packages" </> Parser.string
        , Parser.map OrgPackagesRoute <| Parser.s "packages" </> Parser.string </> Parser.string
        , Parser.map DonateRoute <| Parser.s "donate"
        , Parser.map UsersRoute <| Parser.s "users" </> Parser.string
        , Parser.map SettingsRoute <| Parser.s "settings" </> Parser.string
        , Parser.map SettingRoute <| Parser.s "settings"
        ]


parse : Url.Url -> Route
parse url =
    case Parser.parse matchers url of
        Just route ->
            route

        Nothing ->
            NotFoundRoute
