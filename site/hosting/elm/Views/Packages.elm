module Views.Packages exposing (view)

import Array
import Html exposing (..)
import Html.Attributes exposing (..)
import Html.Events exposing (..)
import Messages exposing (..)
import Model exposing (..)
import Routing exposing (..)
import Views.NotFound as NotFound
import Views.Svgs as Svgs
import Markdown


view : Model -> String -> Html Msg
view model name =
    selectListOrDetail model name


selectListOrDetail : Model -> String -> Html Msg
selectListOrDetail model name =
    div [ class "packages" ]
        [ if String.isEmpty name then
            listView model

          else
            detailView model
        ]


listView : Model -> Html Msg
listView model =
    div []
        [ input
            [ type_ "search"
            , id "search-input"
            , placeholder "Search packages"
            , value model.searchInput
            , onInput OnSearchInput
            ]
            []
        , div []
            [ div [ id "hits" ] []
            , div [ id "pagination" ] []
            ]
        , hitTemplate
        ]


script_ : List (Attribute msg) -> List (Html msg) -> Html msg
script_ =
    Html.node "script"


hitTemplate : Html Msg
hitTemplate =
    script_
        [ type_ "text/html"
        , id "hit-template"
        , hidden True
        ]
        [ div [ class "hit" ]
            [ div [ class "container" ]
                [ div [ class "list-item" ]
                    [ a
                        [ class "hit-name"
                        , href <| Routing.pathFor (PackageRoute "{{{name}}}")
                        ]
                        [ text "{{{name}}}" ]
                    , span [ class "hit-version" ]
                        [ text "{{{version}}}" ]
                    , p [ class "hit-description" ]
                        [ text "{{{description}}}" ]
                    ]
                ]
            ]
        ]


detailView : Model -> Html Msg
detailView model =
    case model.detailedPackage of
        Success detailedPackage ->
            detailMainView detailedPackage model

        Requesting ->
            div [ class "spinner" ]
                [ Svgs.spinner ]

        _ ->
            NotFound.view


getLink : Links -> List (Html Msg)
getLink links =
    case ( links.github, links.homepage ) of
        ( Just github, Just homepage ) ->
            [ li []
                [ span [] [ text "GitHub: " ]
                , a [ href github ] [ text github ]
                ]
            , li []
                [ span [] [ text "Homepage: " ]
                , a [ href homepage ] [ text homepage ]
                ]
            ]

        ( Just github, Nothing ) ->
            [ li []
                [ span [] [ text "GitHub: " ]
                , a [ href github ] [ text github ]
                ]
            ]

        ( Nothing, Just homepage ) ->
            [ li []
                [ span [] [ text "Homepage: " ]
                , a [ href homepage ] [ text homepage ]
                ]
            ]

        ( Nothing, Nothing ) ->
            []


getLinks : Maybe Links -> List (Html Msg)
getLinks links =
    case links of
        Just ln ->
            h3 [] [ text "Links" ]
                :: getLink ln

        Nothing ->
            []


getDep : Dependency -> Html Msg
getDep dep =
    let
        withDefault =
            Maybe.withDefault ""

        name =
            dep.name
                |> String.split "/"
                |> Array.fromList

        route =
            if Array.length name == 2 then
                OrgPackageRoute
                    (withDefault <| Array.get 0 name)
                    (withDefault <| Array.get 1 name)

            else
                PackageRoute
                    (withDefault <| Array.get 0 name)
    in
    li []
        [ a [ href <| Routing.pathFor route ]
            [ text dep.name ]
        , text <| ": " ++ dep.version
        ]


getDeps : Maybe (List Dependency) -> List (Html Msg)
getDeps maybe_deps =
    case maybe_deps of
        Just deps ->
            let
                deps_num =
                    deps
                        |> List.length
                        |> String.fromInt
            in
            h3 [] [ text <| deps_num ++ " Dependencies" ]
                :: List.map getDep deps

        Nothing ->
            [ h3 [] [ text "0 Dependencies" ]
            ]


getLatestVersion : List String -> String
getLatestVersion versions =
    versions
        |> List.head
        -- TODO: head? tail?
        |> Maybe.withDefault ""


getVer : String -> Html Msg
getVer version =
    li [] [ text version ]


getVersions : List String -> Html Msg
getVersions versions =
    let
        vers_num =
            versions
                |> List.length
                |> String.fromInt

        elements =
            h3 [] [ text <| vers_num ++ " Versions" ]
                :: List.map getVer versions
    in
    div [ class "versions" ] elements


detailMainView : DetailedPackage -> Model -> Html Msg
detailMainView detailedPackage model =
    div [ class "container" ]
        [ span [ class "package-title" ]
            [ text detailedPackage.name
            ]
        , span [ class "hit-version" ]
            [ text <| getLatestVersion detailedPackage.versions
            ]
        , div [ class "hit-description" ]
            [ text detailedPackage.description
            ]
        , div [ class "cpp-version" ]
            [ text <| "C++ version: " ++ String.fromInt detailedPackage.cpp_version
            ]
        , div [ class "details" ]
            [ case model.readme of
                Just readme ->
                    Markdown.toHtml [ class "readme" ] readme
                Nothing ->
                    div [ class "readme" ] [] -- TODO: この時必要ない
            , div [ class "dependencies" ] <|
                getDeps detailedPackage.deps

            --        , div [ class "dependents" ] [
            --            h2 [] [ text "Dependents" ]
            --        ]
            , div [ class "config" ]
                [ h3 [] [ text "Config" ]
                , span [] [ text "poac.yml" ]
                , input
                    [ type_ "text"
                    , readonly True
                    , value <| detailedPackage.name ++ ": " ++ getLatestVersion detailedPackage.versions
                    ]
                    []
                ]
            , div [ class "owners" ] <|
                h3 [] [ text "Owners" ]
                    :: List.map ownersMap detailedPackage.owners
            , div [ class "checksum" ]
                [ h3 [] [ text "Checksum" ]
                , input
                    [ type_ "text"
                    , readonly True
                    , value detailedPackage.md5hash
                    , style "width" "200px"
                    ]
                    []
                ]
            , div [ class "links" ] <|
                getLinks detailedPackage.links

            --            Download .tar.gz, detailedPackage.object_link
            , div [ class "license" ]
                [ h3 [] [ text "License" ]
                , text <| Maybe.withDefault "None" detailedPackage.license
                ]
            , getVersions detailedPackage.versions
            , div [ class "created-date" ]
                [ h3 [] [ text "Created date" ]
                , text detailedPackage.created_date
                ]
            ]
        ]


ownersMap : String -> Html Msg
ownersMap owner =
    li []
        [ a
            [ href <| Routing.pathFor (UsersRoute owner)
            , style "cursor" "pointer"
            ]
            [ text owner
            ]
        ]
