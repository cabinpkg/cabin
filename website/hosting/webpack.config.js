const path = require('path');
const ExtractTextPlugin = require('extract-text-webpack-plugin');
const UglifyJsPlugin = require('uglifyjs-webpack-plugin');
const OptimizeCSSAssetsPlugin = require('optimize-css-assets-webpack-plugin');
const CopyWebpackPlugin = require('copy-webpack-plugin');


const appCss = new ExtractTextPlugin({
    filename: '../css/app.css'
});
const appMobileCss = new ExtractTextPlugin({
    filename: '../css/app_mobile.css'
});


module.exports = {
    optimization: {
        minimizer: [
            new UglifyJsPlugin({
                cache: true,
                parallel: true,
                sourceMap: true
            }),
            new OptimizeCSSAssetsPlugin({})
        ]
    },

    entry: './js/app.js',

    output: {
        path: path.resolve(__dirname, '../dist/js'),
        filename: 'app.js'
    },

    plugins: [
        appCss,
        appMobileCss,
        new CopyWebpackPlugin([{
            from: 'assets/',
            to: '../'
        }]),
        new CopyWebpackPlugin([{
            from: 'index.html',
            to: '../'
        }])
    ],

    module: {
        rules: [
            {
                test: /\.js$/,
                exclude: /node_modules/,
                use: {
                    loader: 'babel-loader'
                }
            },
            {
                test: /\.scss$/,
                exclude: [/_mobile\.scss$/],
                use: appCss.extract({
                    use: [
                        {
                            loader: "css-loader",
                            options: {
                                url: false,
                                localIdentName: '[local]'
                            }
                        },
                        "sass-loader"
                    ]
                })
            },
            {
                test: /\.scss$/,
                include: [/_mobile\.scss$/],
                use: appMobileCss.extract({
                    use: [
                        {
                            loader: "css-loader",
                            options: {
                                url: false,
                                localIdentName: '[local]'
                            }
                        },
                        "sass-loader"
                    ]
                })
            },
            {
                test: /\.elm$/,
                exclude: [/elm-stuff/, /node_modules/],
                loader: "elm-webpack-loader",
                options: {
                    optimize: true
                }
            },
        ],
        noParse: [/\.elm$/]
    }
};
