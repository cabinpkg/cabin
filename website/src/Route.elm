module Route exposing (Route(..), fromUrl, href, parser, replaceUrl)

import Browser.Navigation as Nav
import Html.Styled exposing (..)
import Html.Styled.Attributes as Attributes
import Url exposing (Url)
import Url.Parser as Parser exposing ((</>), (<?>), Parser, oneOf, string)
import Url.Parser.Query as Query



-- ROUTING


type Route
    = Home
    | Packages (Maybe Int)
    | Policies
    | Policy String
    | NotFound


parser : Parser (Route -> a) a
parser =
    oneOf
        [ Parser.map Home Parser.top
        , Parser.map Packages (Parser.s "packages" <?> Query.int "p")
        , Parser.map Policies (Parser.s "policies")
        , Parser.map Policy (Parser.s "policies" </> string)
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
        pieces : List String
        pieces =
            case page of
                Home ->
                    []

                Packages Nothing ->
                    [ "packages" ]

                Packages (Just p) ->
                    [ "packages?p=" ++ String.fromInt p ]

                Policies ->
                    [ "policies" ]

                Policy name ->
                    [ "policies", name ]

                NotFound ->
                    [ "not-found" ]
    in
    "/" ++ String.join "/" pieces
