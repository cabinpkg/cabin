module Algolia.Elements exposing (..)

import Css exposing (..)
import Html.Styled exposing (..)
import Html.Styled.Attributes exposing (autocomplete, css, for, id, placeholder, spellcheck, type_)
import Messages exposing (Msg)


searchBox : Style -> List (Attribute Msg) -> Html Msg
searchBox style attrs =
    input
        (css [ style ]
            :: type_ "search"
            :: id "aa-search-input"
            :: placeholder "Search packages"
            :: autocomplete False
            :: spellcheck False
            :: attrs
        )
        []


searchBoxLabel : Html Msg
searchBoxLabel =
    label
        [ for "aa-search-input"
        , css
            [ visibility hidden
            , display block
            ]
        ]
        [ text "Search packages" ]
