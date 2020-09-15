module Page.Header exposing (view)

import Assets
import Css exposing (..)
import Css.Global as Global
import GlobalCss exposing (..)
import Html.Styled exposing (..)
import Html.Styled.Attributes exposing (css, href)
import Messages exposing (..)
import Model exposing (Model)
import Route exposing (Route)


view : Model -> Html Msg
view model =
    header
        [ css
            [ width (vw 80)
            , height (px 74)
            , marginLeft auto
            , marginRight auto
            ]
        ]
        [ div
            [ css
                [ legacyDisplayFlex
                , height (px 74)
                , legacyAlignItems "center"
                , legacyJustifyContentSpaceBetween
                , Global.children
                    [ Global.everything
                        [ unselectable
                        ]
                    ]
                ]
            ]
            [ logo model
            , headerMenu model
            ]
        ]


logoStyle : Model -> Style
logoStyle model =
    Css.batch <|
        if model.width < 500 then
            [ width (px 30)
            , padding zero
            ]

        else
            [ fontWeight (int 900)
            , fontStyle normal
            , fontSize (px 10)
            , letterSpacing (px 1.25)
            , lineHeight (px 12)
            , textDecoration none
            , padding (px 20)
            , legacyTransition "0.3s"
            ]


logo : Model -> Html Msg
logo model =
    a
        [ Route.href Route.Home
        , css
            [ visibility hidden
            , logoStyle model
            ]
        ]
        [ text "poac"
        , div
            [ css [ visibility visible ] ]
            [ Assets.logo model ]
        ]


headerMenu : Model -> Html Msg
headerMenu model =
    nav []
        [ ul
            [ css <|
                if model.width < 500 then
                    [ padding zero ]

                else
                    []
            ]
          <|
            List.map toLi
                [ headerItemPackages
                , headerItemDocs
                ]
        ]


headerItemLiStyled : List (Attribute msg) -> List (Html msg) -> Html msg
headerItemLiStyled =
    styled li
        [ listStyle none
        , paddingRight (px 30)
        , display tableCell
        , verticalAlign middle
        , textAlign center
        ]


toLi : Html Msg -> Html Msg
toLi item =
    headerItemLiStyled [] [ item ]


headerItemAStyled : List (Attribute msg) -> List (Html msg) -> Html msg
headerItemAStyled =
    styled a
        [ textDecoration none
        , fontSize (px 10)
        , fontStyle normal
        , fontWeight (int 900)
        , letterSpacing (px 1.25)
        , lineHeight (px 12)
        , padding (px 20)
        ]


headerItemPackages : Html Msg
headerItemPackages =
    headerItemAStyled
        [ Route.href Route.Packages ]
        [ text "PACKAGES" ]


headerItemDocs : Html Msg
headerItemDocs =
    headerItemAStyled
        [ href "https://doc.poac.pm/" ]
        [ text "DOCS" ]
