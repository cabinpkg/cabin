module Routing exposing (Route(..), matchers, parseUrl, pathFor)

import Url exposing (Url)
import Url.Parser exposing (..)


type Route
    = HomeIndexRoute
    | PackagesRoute
    | PackageRoute String
    | OrgPackageRoute String String
    | PricingRoute
    | UsersRoute String
    | SettingRoute
    | SettingsRoute String
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
        , map PricingRoute (s "pricing")
        , map UsersRoute (s "users" </> string)
        , map SettingRoute (s "settings")
        , map SettingsRoute (s "settings" </> string)
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

        PricingRoute ->
            "/pricing"

        UsersRoute userId ->
            "/users/" ++ userId

        SettingRoute ->
            "/settings"
        SettingsRoute menu ->
            "/settings/" ++ menu

        PolicyRoute ->
            "/policies"
        PoliciesRoute name ->
            "/policies/" ++ name

        NotFoundRoute ->
            "/not-found"
