module Route exposing (Route(..), parser, href, replaceUrl, fromUrl)

import Browser.Navigation as Nav
import Html.Styled exposing (..)
import Html.Styled.Attributes as Attributes
import Url exposing (Url)
import Url.Parser as Parser exposing ((</>), Parser, oneOf, string)



-- ROUTING


type Route
    = Home
    | Packages
    | Policy
    | Policies String
    | NotFound


parser : Parser (Route -> a) a
parser =
    oneOf
        [ Parser.map Home Parser.top
        , Parser.map Packages (Parser.s "packages")
        , Parser.map Policy (Parser.s "policies")
        , Parser.map Policies (Parser.s "policies" </> string)
        ]



-- PUBLIC HELPERS


href : Route -> Attribute msg
href targetRoute =
    Attributes.href (routeToString targetRoute)


replaceUrl : Nav.Key -> Route -> Cmd msg
replaceUrl key route =
    Nav.replaceUrl key (routeToString route)


fromUrl : Url -> Route
fromUrl url =
    case Parser.parse parser url of
        Just route ->
            route

        Nothing ->
            NotFound



-- INTERNAL


routeToString : Route -> String
routeToString page =
    let
        pieces =
            case page of
                Home ->
                    []

                Packages ->
                    [ "packages" ]

                Policy ->
                    [ "policies" ]

                Policies name ->
                    [ "policies", name ]

                NotFound ->
                    [ "not-found" ]
    in
    "/" ++ String.join "/" pieces
