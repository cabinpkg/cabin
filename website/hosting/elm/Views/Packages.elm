module Views.Packages exposing (view)

import Views.NotFound as NotFound
import Views.Svgs as Svgs
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
            div [ class "spinner" ]
                [ Svgs.spinner ]
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
            mainView model (div [ class "spinner" ]
                                [ Svgs.spinner ]
                           )
        _ ->
            NotFound.view


getLink : Links -> List (Html Msg)
getLink links =
    case (links.github, links.homepage) of
        (Just github, Just homepage) ->
            [ li []
              [ span [] [ text "GitHub: " ]
              , a [ href github ] [ text github ]
              ]
            , li []
              [ span [] [ text "Homepage: " ]
              , a [ href homepage ] [ text homepage ]
              ]
            ]
        (Just github, Nothing) ->
            [ li []
              [ span [] [ text "GitHub: " ]
              , a [ href github ] [ text github ]
              ]
            ]
        (Nothing, Just homepage) ->
            [ li []
              [ span [] [ text "Homepage: " ]
              , a [ href homepage ] [ text homepage ]
              ]
            ]
        (Nothing, Nothing) ->
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
                h3 [] [ text <| deps_num ++ " Dependencies" ]
                :: List.map getDep deps
        Nothing ->
            [ h3 [] [ text "0 Dependencies" ]
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
        [ span [ class "package-title" ]
            [ text detailedPackage.name
            ]
        , span [ class "version" ]
            [ text <| getLatestVersion detailedPackage.versions
            ]
        , div [ class "description" ]
            [ text detailedPackage.description
            ]
        , div [ class "details" ]
        [ div [ class "dependencies" ]
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
            , value <| detailedPackage.name ++ ": " ++ (getLatestVersion detailedPackage.versions)
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
    li [] [
        a [ onClick <| NavigateTo (UsersRoute owner)
          , style [("cursor", "pointer")] ] [
            text owner
        ]
    ]
