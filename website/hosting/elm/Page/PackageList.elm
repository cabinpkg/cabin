module Page.PackageList exposing (..)

import Html exposing (..)
import Html.Attributes exposing (..)
import Html.Events exposing (..)
import Model exposing (..)
import Messages exposing (..)
import Route


view : Model -> Html Msg
view model =
    main_
        [ class "packages" ]
        [ listView model ]


listView : Model -> Html Msg
listView model =
    div []
        [ div [ class "search-refinement-list" ]
              [ div [ id "cpp-version" ] []
              , div [ id "package-type" ] []
              ]
        , div [ class "search-results" ]
              [ input
                  [ type_ "search"
                  , id "search-input"
                  , placeholder "Search packages"
                  , value model.searchInput
                  , onInput OnSearchInput
                  ]
                  []
              , div [ id "search-top-container" ] []
              , div []
                  [ div [ id "hits" ] []
                  , div [ id "pagination" ] []
                  ]
              , hitTemplate
              ]
        ]


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
                        , Route.href (Route.Package "{{owner}}" "{{repo}}" "latest")
                        ]
                        [ text "{{{owner}}}/{{repo}}" ]
                    , span [ class "hit-version" ]
                        [ text "{{{version}}}" ]
                    , span [ class "hit-package_type" ]
                        [ text "{{{package_type}}}" ]
                    , p [ class "hit-description" ]
                        [ text "{{{description}}}" ]
                    ]
                ]
            ]
        ]


script_ : List (Attribute msg) -> List (Html msg) -> Html msg
script_ =
    Html.node "script"
