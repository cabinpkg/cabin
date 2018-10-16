module Views.Packages exposing (view)

import Views.NotFound as NotFound
import Html exposing (..)
import Html.Attributes exposing (..)
import Html.Events exposing (..)
import Messages exposing (..)
import Routing exposing (..)
import Model exposing (..)


view : Model -> String -> Html Msg
view model name =
    selectListOrDetail model name

mainView : Model -> Html Msg -> Html Msg
mainView model element =
    div [ class "packages" ] [
        element
    ]

selectListOrDetail : Model -> String -> Html Msg
selectListOrDetail model name =
    if String.isEmpty name then
        listView model
    else
        detailView model


listView : Model -> Html Msg
listView model =
    mainView model <|
    case model.listPackages of
        Success listPackages ->
            div [] [
              div [ class "search-top-container" ] [
--                div [ class "search-top-inline" ] [
                  text <| (toString <| List.length listPackages) ++ " packages found"
--                ]
              ]
              , div [ class "container" ] (
                List.map listItemView listPackages
              )
            ]
        Requesting ->
            a [] [ text "Loading..." ]
        _ ->
            NotFound.view

listItemView : Package -> Html Msg
listItemView package =
    div [ class "list-item" ] [
        a [ class "packname"
          , onClick <| NavigateTo (PackagesRoute package.name) -- TODO: NavigateTo
        ] [ text package.name ]
        , span [ class "version" ] [ text package.version ]
        , p [ class "description" ] [ text package.description ]
    ]

replace : String -> String -> String -> String
replace from to str =
    String.split from str
        |> String.join to


detailView : Model -> Html Msg
detailView model =
    case model.detailedPackage of
        Success detailedPackage ->
            mainView model (detailMainView detailedPackage)
        Requesting ->
            mainView model (a [] [ text "Loading..." ])
        _ ->
            NotFound.view


getLink : Links -> List (Html Msg)
getLink links =
    case (links.github, links.homepage) of
        (Just github, Just homepage) ->
            [ li [] [ a [ href github ] [ text <| "GitHub: " ++ github ] ]
            , li [] [ a [ href homepage ] [ text <| "Homepage: " ++ homepage ] ]
            ]
        (Just github, Nothing) ->
            [ a [ href github ] [ text <| "GitHub: " ++ github ] ]
        (Nothing, Just homepage) ->
            [ a [ href homepage ] [ text <| "Homepage: " ++ homepage ] ]
        (Nothing, Nothing) ->
            []

getLinks : Maybe Links -> List (Html Msg)
getLinks links =
    case links of
        Just ln ->
            getLink ln
        Nothing ->
            []


getDep : Dependency -> Html Msg
getDep dep =
    li [] [ text <| dep.name ++ ": " ++ dep.version ]

getDeps : Maybe (List Dependency) -> List (Html Msg)
getDeps maybe_deps =
    case maybe_deps of
        Just deps ->
            let
                deps_num =
                    deps
                    |> List.length
                    |> toString
            in
                h2 [] [ text <| deps_num ++ " Dependencies" ]
                :: List.map getDep deps
        Nothing ->
            [ h2 [] [ text "0 Dependencies" ]
            ]

getLatestVersion : List String -> String
getLatestVersion versions =
    versions
    |> List.head -- TODO: head? tail?
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
            |> toString
        elements =
            h3 [] [ text <| vers_num ++ " Versions" ]
            :: (List.map getVer versions)
    in
        div [ class "versions" ] elements

detailMainView : DetailedPackage -> Html Msg
detailMainView detailedPackage =
    div [ class "container" ]
        [ h2 [ class "package-title" ]
            [ text detailedPackage.name
            ]
        , span [ class "version" ]
            [ text <| getLatestVersion detailedPackage.versions
            ]
        , div [ class "description" ]
            [ text detailedPackage.description
            ]
        , div [ class "dependencies" ]
          <| getDeps detailedPackage.deps
--        , div [ class "dependents" ] [
--            h2 [] [ text "Dependents" ]
--        ]
        , div [ class "config" ]
          [ h3 [] [ text "Config" ]
          , span [] [ text "poac.yml" ]
          , input
            [ type_ "text"
            , readonly True
            , value <| detailedPackage.name ++ ": " ++ "0.1"
            ] []
          ]
        , div [ class "owners" ]
          <| h3 [] [ text "Owners" ]
             :: List.map ownersMap detailedPackage.owners
        , div [ class "checksum" ]
          [ h3 [] [ text "Checksum" ]
          , input
            [ type_ "text"
            , readonly True
            , value detailedPackage.md5hash
            , style [("width", "200px")]
            ] []
        ]
        , div [ class "links" ]
            <| getLinks detailedPackage.links
        , div [ class "license" ]
            [ h3 [] [ text "License" ]
            , text <| Maybe.withDefault "None" detailedPackage.license
            ]
        , getVersions detailedPackage.versions
    ]

ownersMap : String -> Html Msg
ownersMap owner =
    li [] [
        a [ onClick <| NavigateTo (UsersRoute owner)
          , style [("cursor", "pointer")] ] [
            text owner
        ]
    ]
